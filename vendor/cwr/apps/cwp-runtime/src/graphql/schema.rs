mod conversations;

use super::conversation_summary::ConversationRowSummary;
use async_graphql::{Context, Object, Result, SimpleObject, Union, ID};
use cwr::v2026_09::{
    conversation::{
        ConversationSummary, ConversationSummaryKey, ConversationSummaryPageRequest,
        ConversationTimelineAnchor,
    },
    local::LocalRuntime,
};
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone)]
pub(super) struct ReadService {
    pub action_titles: std::sync::Arc<tokio::sync::OnceCell<BTreeMap<String, String>>>,
    pub configurations: super::coworkers::ConfigurationSnapshot,
    pub runtime: LocalRuntime,
    pub publisher: super::AdmissionPublisher,
    pub runtime_ref: String,
    pub actor_ref: String,
    pub owner: Option<String>,
    pub ingress: String,
    pub budget: Option<super::budget::ReadBudget>,
    pub reads: std::sync::Arc<tokio::sync::Semaphore>,
    pub publications: super::publications::View,
}

impl ReadService {
    pub fn fresh_selection(&self) -> Self {
        let mut service = self.clone();
        service.publications = Default::default();
        service.configurations = Default::default();
        service.action_titles = Default::default();
        service.budget = Some(super::budget::ReadBudget::new(Duration::from_secs(10)));
        service
    }
    pub fn budget(&self) -> super::budget::ReadBudget {
        self.budget
            .clone()
            .expect("resource read belongs to a selection")
    }
    pub async fn action_title(&self, activation: &str) -> Result<Option<String>> {
        let titles = self
            .action_titles
            .get_or_try_init(|| async {
                let service = self.clone();
                blocking(service.reads.clone(), service.budget(), move || {
                    Ok(service
                        .catalog()?
                        .actions
                        .into_iter()
                        .map(|action| (action.activation, action.title))
                        .collect())
                })
                .await
            })
            .await?;
        Ok(titles.get(activation).cloned())
    }
    pub fn catalog(&self) -> cwr::CwrResult<cwr::v2026_09::local::LocalCatalog> {
        let budget = self.budget();
        self.runtime.upstream_catalog_as_until(
            &self.actor_ref,
            self.owner.as_deref(),
            budget.deadline(),
            || budget.cancelled(),
        )
    }
}

pub(super) struct Query;
#[Object]
impl Query {
    async fn runtime(&self, ctx: &Context<'_>) -> Runtime {
        Runtime(ctx.data_unchecked::<ReadService>().clone())
    }
}

#[derive(Clone)]
pub(super) struct Runtime(pub(super) ReadService);
#[Object]
impl Runtime {
    async fn file(
        &self,
        #[graphql(name = "ref")] reference: ID,
    ) -> Result<super::files::FileContent> {
        super::files::read(self.0.clone(), reference.to_string()).await
    }

    async fn message(
        &self,
        #[graphql(name = "ref")] reference: ID,
    ) -> Result<Option<super::publications::MessageInterface>> {
        super::publications::lookup_message(self.0.clone(), reference.to_string()).await
    }
    async fn timeline_item(
        &self,
        #[graphql(name = "ref")] reference: ID,
    ) -> Result<Option<super::publications::TimelineItem>> {
        super::publications::lookup_item(self.0.clone(), reference.to_string()).await
    }
    async fn node(
        &self,
        #[graphql(name = "ref")] reference: ID,
    ) -> Result<Option<super::publications::Node>> {
        super::publications::lookup_node(self.0.clone(), reference.to_string()).await
    }
    pub(super) async fn id(&self) -> ID {
        ID(self.0.runtime_ref.clone())
    }
    /// Session-visible targets intersected with the host product context.
    async fn message_targets(
        &self,
        owner_context_ref: Option<ID>,
    ) -> Result<Vec<super::messages::Coworker>> {
        super::contexts::message_targets(self.0.clone(), owner_context_ref).await
    }
    async fn default_message_target(
        &self,
        owner_context_ref: Option<ID>,
    ) -> Result<Option<super::messages::Coworker>> {
        super::contexts::default_message_target(self.0.clone(), owner_context_ref).await
    }
    async fn actions(
        &self,
        owner_context_ref: Option<ID>,
    ) -> Result<Vec<super::mutations::Action>> {
        let service = self.0.clone();
        blocking(service.reads.clone(), service.budget(), move || {
            let available = service
                .runtime
                .conversation_context_targets(owner_context_ref.as_ref().map(|id| id.as_str()))?;
            Ok(service
                .catalog()?
                .actions
                .into_iter()
                .filter(|action| {
                    action.actor_ref == cwr::action_contract::INTRINSIC_CWR_ACTOR_REF
                        || available
                            .as_ref()
                            .is_none_or(|allowed| allowed.contains(&action.actor_ref))
                })
                .map(Into::into)
                .collect())
        })
        .await
    }
    /// The installed-schedules-v1 collection for one host-owned pack context.
    async fn schedules(
        &self,
        owner_context_ref: ID,
    ) -> Result<Vec<super::schedules::InstalledScheduleV1>> {
        super::schedules::list(self.0.clone(), owner_context_ref).await
    }
    async fn schedule_runs(
        &self,
        owner_context_ref: ID,
        schedule_ref: ID,
        #[graphql(default = 20)] first: i32,
    ) -> Result<Vec<super::schedules::ScheduleOccurrenceV1>> {
        super::schedules::runs(self.0.clone(), owner_context_ref, schedule_ref, first).await
    }
    async fn invocation(
        &self,
        id: Option<ID>,
        #[graphql(name = "ref")] reference: Option<ID>,
    ) -> Result<Option<super::mutations::ActionInvocation>> {
        let id = reference
            .or(id)
            .ok_or_else(|| async_graphql::Error::new("A canonical ref is required"))?;
        let service = self.0.clone();
        blocking(service.reads.clone(), service.budget(), move || {
            super::mutations::read_invocation(&service, id.as_str())
        })
        .await
    }

    async fn actor(&self) -> Result<super::messages::Actor> {
        let service = self.0.clone();
        blocking(service.reads.clone(), service.budget(), move || {
            service
                .publications
                .read(&service, |view| view.actor(&service.actor_ref))?
                .map(super::messages::actor)
                .ok_or_else(|| std::io::Error::other("Session actor unavailable").into())
        })
        .await
    }
    async fn actor_ref(&self) -> ID {
        ID(self.0.actor_ref.clone())
    }

    async fn conversations(
        &self,
        #[graphql(default = 50)] first: i32,
        after: Option<String>,
        #[graphql(default = false)] include_archived: bool,
        owner_context_ref: Option<ID>,
        #[graphql(default = false)] include_legacy_owner: bool,
    ) -> Result<ConversationConnection> {
        let count = page_size(first)?;
        let after = after
            .map(|s| serde_json::from_str::<ConversationSummaryKey>(&s))
            .transpose()?;
        let service = self.0.clone();
        blocking(service.reads.clone(), service.budget(), move || {
            let diagnostic_started = (std::env::var_os("WLP_CWP_ROUTE_TIMING")
                .as_deref()
                == Some(std::ffi::OsStr::new("1")))
            .then(std::time::Instant::now);
            service.catalog()?;
            let default_context = service.runtime.default_conversation_context()?;
            let selected_context = owner_context_ref
                .map(|id| id.to_string())
                .or_else(|| default_context.clone());
            let include_legacy_owner =
                if selected_context.is_some() && selected_context == default_context {
                    true
                } else {
                    include_legacy_owner
                };
            if include_legacy_owner && selected_context != default_context {
                return Err(std::io::Error::other(
                    "Legacy conversations belong to the default context",
                )
                .into());
            }
            service
                .runtime
                .conversation_context_targets(selected_context.as_deref())?;
            let page = service.publications.read(&service, |view| {
                view.conversations(
                    if include_archived { None } else { Some(false) },
                    &ConversationSummaryPageRequest {
                        owner_context_ref: selected_context,
                        include_legacy_owner,
                        first: Some(count),
                        after,
                        ..Default::default()
                    },
                )
            })?;
            if let Some(started) = diagnostic_started {
                eprintln!(
                    "{{\"event\":\"cwp_runtime_conversations_timing\",\"elapsedMs\":{},\"rowCount\":{},\"requestedFirst\":{},\"includeArchived\":{}}}",
                    started.elapsed().as_millis(),
                    page.summaries.len(),
                    count,
                    include_archived,
                );
            }
            let edges = page
                .summaries
                .into_iter()
                .map(|item| {
                    Ok(ConversationEdge {
                        cursor: serde_json::to_string(&item.key)?,
                        node: Conversation {
                            service: service.clone(),
                            id: item.key.conversation_ref.clone(),
                            title: item.title.clone(),
                            archived: item.archived,
                            summary: Some(item),
                        },
                    })
                })
                .collect::<cwr::CwrResult<Vec<_>>>()?;
            Ok(ConversationConnection {
                page_info: PageInfo::new(
                    page.has_next_page,
                    page.has_previous_page,
                    edges.first().map(|e| e.cursor.clone()),
                    edges.last().map(|e| e.cursor.clone()),
                ),
                edges,
                total_count: page.total_count as i32,
                revision: page.revision,
            })
        })
        .await
    }

    async fn conversation(
        &self,
        id: Option<ID>,
        #[graphql(name = "ref")] reference: Option<ID>,
    ) -> Result<Option<Conversation>> {
        let id = reference
            .or(id)
            .ok_or_else(|| async_graphql::Error::new("A canonical ref is required"))?;
        let service = self.0.clone();
        blocking(service.reads.clone(), service.budget(), move || {
            service.catalog()?;
            let value = service
                .publications
                .read(&service, |view| view.conversation(id.as_str()))?;
            Ok(value.map(|(title, archived)| Conversation {
                service: service.clone(),
                id: id.to_string(),
                title,
                archived,
                summary: None,
            }))
        })
        .await
    }
}

#[derive(Clone)]
pub(super) struct Conversation {
    service: ReadService,
    id: String,
    title: Option<String>,
    archived: bool,
    summary: Option<ConversationSummary>,
}
impl Conversation {
    pub(super) fn from_snapshot(
        service: ReadService,
        id: String,
        title: Option<String>,
        archived: bool,
    ) -> Self {
        Self {
            service,
            id,
            title,
            archived,
            summary: None,
        }
    }
}

#[derive(SimpleObject)]
struct ConversationConnection {
    edges: Vec<ConversationEdge>,
    page_info: PageInfo,
    total_count: i32,
    revision: String,
}
#[derive(SimpleObject)]
struct ConversationEdge {
    cursor: String,
    node: Conversation,
}
#[derive(SimpleObject)]
struct TimelineConnection {
    edges: Vec<TimelineEdge>,
    page_info: PageInfo,
    total_count: i32,
    revision: String,
}
#[derive(SimpleObject)]
struct TimelineEdge {
    cursor: String,
    node: TimelineNode,
}
#[derive(Union)]
enum TimelineNode {
    Message(Message),
    Admitted(super::invocations::AdmittedInvocation),
    AwaitingPerson(super::invocations::AwaitingPersonInvocation),
    Running(super::invocations::RunningInvocation),
    Completed(super::invocations::CompletedInvocation),
    Blocked(super::invocations::BlockedInvocation),
    Failed(super::invocations::FailedInvocation),
    Stopping(super::invocations::StoppingInvocation),
    Stopped(super::invocations::StoppedInvocation),
}
impl From<super::invocations::ActionInvocation> for TimelineNode {
    fn from(value: super::invocations::ActionInvocation) -> Self {
        match value {
            super::invocations::ActionInvocation::Admitted(value) => Self::Admitted(value),
            super::invocations::ActionInvocation::AwaitingPerson(value) => {
                Self::AwaitingPerson(value)
            }
            super::invocations::ActionInvocation::Running(value) => Self::Running(value),
            super::invocations::ActionInvocation::Completed(value) => Self::Completed(value),
            super::invocations::ActionInvocation::Blocked(value) => Self::Blocked(value),
            super::invocations::ActionInvocation::Failed(value) => Self::Failed(value),
            super::invocations::ActionInvocation::Stopping(value) => Self::Stopping(value),
            super::invocations::ActionInvocation::Stopped(value) => Self::Stopped(value),
        }
    }
}
#[derive(Clone, SimpleObject)]
#[graphql(name = "PublishedMessage", complex)]
pub(super) struct Message {
    #[graphql(skip)]
    pub(super) service: ReadService,
    #[graphql(owned)]
    pub(super) id: ID,
    #[graphql(owned)]
    pub(super) author_actor_ref: ID,
    #[graphql(owned)]
    pub(super) addressed_actor_refs: Vec<ID>,
    #[graphql(owned)]
    pub(super) reply_to_message_ref: Option<ID>,
    #[graphql(owned)]
    pub(super) text: String,
}
#[derive(SimpleObject)]
pub(super) struct PageInfo {
    has_next_page: bool,
    has_previous_page: bool,
    start_cursor: Option<super::publications::Cursor>,
    end_cursor: Option<super::publications::Cursor>,
}
impl PageInfo {
    pub(super) fn new(
        has_next_page: bool,
        has_previous_page: bool,
        start_cursor: Option<String>,
        end_cursor: Option<String>,
    ) -> Self {
        Self {
            has_next_page,
            has_previous_page,
            start_cursor: start_cursor.map(super::publications::Cursor),
            end_cursor: end_cursor.map(super::publications::Cursor),
        }
    }
}
fn page_size(value: i32) -> Result<usize> {
    if !(1..=100).contains(&value) {
        return Err("first must be between 1 and 100".into());
    }
    Ok(value as usize)
}
pub(super) use super::budget::blocking;
