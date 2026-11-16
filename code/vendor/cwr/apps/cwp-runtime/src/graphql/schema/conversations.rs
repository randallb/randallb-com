//! Conversation fields share the authorized publication selection.
use super::*;
use crate::graphql::{contexts, legacy_history, messages, mutations, publications};
use cwr::v2026_09::conversation::{ConversationPositionKey, ConversationRecordPageRequest};

#[derive(async_graphql::SimpleObject)]
struct ForkSource {
    conversation_ref: ID,
    message_ref: ID,
}
#[Object]
impl Conversation {
    /// Runtime row metadata from the authorized page or an authorized point read.
    async fn summary(&self) -> Result<Option<ConversationRowSummary>> {
        if let Some(value) = &self.summary {
            return Ok(Some(value.clone().into()));
        }
        let service = self.service.clone();
        let reference = self.id.clone();
        blocking(service.reads.clone(), service.budget(), move || {
            Ok(service
                .publications
                .read(&service, |view| view.conversation_summary(&reference))?
                .map(Into::into))
        })
        .await
    }
    async fn message_targets(&self, ctx: &Context<'_>) -> Result<Vec<messages::Coworker>> {
        let context = self.owner_context_ref(ctx).await?;
        contexts::message_targets(self.service.clone(), context).await
    }
    async fn default_message_target(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<messages::Coworker>> {
        let context = self.owner_context_ref(ctx).await?;
        contexts::default_message_target(self.service.clone(), context).await
    }
    async fn owner_context_ref(&self) -> Result<Option<ID>> {
        let service = self.service.clone();
        let reference = self.id.clone();
        blocking(service.reads.clone(), service.budget(), move || {
            Ok(service
                .publications
                .read(&service, |view| view.owner_context_ref(&reference))?
                .map(ID))
        })
        .await
    }
    /// Recipients of the authenticated actor's latest addressed message.
    async fn last_addressed_actor_refs(&self) -> Result<Vec<ID>> {
        let service = self.service.clone();
        let id = self.id.clone();
        blocking(service.reads.clone(), service.budget(), move || {
            Ok(service
                .publications
                .read(&service, |view| view.last_addressed_actor_refs(&id))?
                .into_iter()
                .map(ID)
                .collect())
        })
        .await
    }
    async fn fork_source(&self) -> Result<Option<ForkSource>> {
        let service = self.service.clone();
        let id = self.id.clone();
        blocking(service.reads.clone(), service.budget(), move || {
            let value = service
                .publications
                .read(&service, |view| view.fork_source(&id))?;
            Ok(value.map(|(conversation, message)| ForkSource {
                conversation_ref: ID(conversation),
                message_ref: ID(message),
            }))
        })
        .await
    }
    /// Nullable canonical layout; an empty retained root remains authoritative.
    async fn canonical_timeline(
        &self,
        #[graphql(default = 50)] last: i32,
        before: Option<publications::Cursor>,
    ) -> Result<Option<publications::TimelineItemConnection>> {
        let service = self.service.clone();
        let reference = self.id.clone();
        let canonical = blocking(service.reads.clone(), service.budget(), move || {
            service
                .publications
                .read(&service, |view| view.timeline_layout(&reference))
        })
        .await?;
        if canonical.is_none() {
            return Ok(None);
        }
        publications::page(
            self.service.clone(),
            self.id.clone(),
            None,
            None,
            None,
            None,
            Some(last),
            before,
            false,
        )
        .await
    }
    /// Selected legacy anchors and their content, without total counts or dependencies.
    async fn legacy_history(
        &self,
        #[graphql(default = 50)] last: i32,
        before: Option<publications::Cursor>,
    ) -> Result<Option<legacy_history::LegacyHistoryPage>> {
        legacy_history::page(self.service.clone(), self.id.clone(), last, before).await
    }
    async fn timeline(
        &self,
        ctx: &Context<'_>,
        filter: Option<publications::TimelineFilter>,
        first: Option<i32>,
        after: Option<publications::Cursor>,
        last: Option<i32>,
        before: Option<publications::Cursor>,
    ) -> Result<publications::TimelineItemConnection> {
        publications::page(
            self.service.clone(),
            self.id.clone(),
            None,
            filter,
            first,
            after,
            last,
            before,
            ctx.look_ahead().field("totalCount").exists(),
        )
        .await?
        .ok_or_else(|| "Publication timeline is unavailable".into())
    }
    pub(in crate::graphql) async fn id(&self) -> ID {
        ID(self.id.clone())
    }
    async fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    async fn archived(&self) -> bool {
        self.archived
    }
    async fn legacy_timeline(
        &self,
        #[graphql(default = 50)] first: i32,
        after: Option<String>,
    ) -> Result<TimelineConnection> {
        let count = page_size(first)?;
        let after = after
            .map(|s| serde_json::from_str::<ConversationPositionKey>(&s))
            .transpose()?;
        let service = self.service.clone();
        let id = self.id.clone();
        blocking(service.reads.clone(), service.budget(), move || {
            service.catalog()?;
            let page = service.publications.read(&service, |view| {
                view.legacy_timeline(
                    &id,
                    &ConversationRecordPageRequest {
                        first: Some(count),
                        after,
                        ..Default::default()
                    },
                )
            })?;
            let messages = page
                .messages
                .into_iter()
                .map(|m| {
                    (
                        m.message_ref.clone(),
                        Message {
                            service: service.clone(),
                            id: ID(m.message_ref),
                            author_actor_ref: ID(m.author_actor_ref),
                            addressed_actor_refs: m
                                .addressed_actor_refs
                                .into_iter()
                                .map(ID)
                                .collect(),
                            reply_to_message_ref: m.reply_to_message_ref.map(ID),
                            text: m.text,
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let edges = page
                .anchors
                .into_iter()
                .map(|anchor| {
                    let (mut key, node) = match anchor {
                        ConversationTimelineAnchor::Message { key, message_ref } => {
                            let message = messages.get(&message_ref).cloned().ok_or_else(|| {
                                std::io::Error::other("timeline message is missing")
                            })?;
                            (key, TimelineNode::Message(message))
                        }
                        ConversationTimelineAnchor::Turn {
                            key,
                            invocation_ref,
                        } => (
                            key,
                            TimelineNode::from(
                                mutations::read_invocation(&service, &invocation_ref)?.ok_or_else(
                                    || std::io::Error::other("timeline invocation is unavailable"),
                                )?,
                            ),
                        ),
                    };
                    key.revision = Some(page.revision.clone());
                    Ok(TimelineEdge {
                        cursor: serde_json::to_string(&key)?,
                        node,
                    })
                })
                .collect::<cwr::CwrResult<Vec<_>>>()?;
            Ok(TimelineConnection {
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
}
