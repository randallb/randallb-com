//! GraphQL projects immutable, authorized CWR publication reads.
use super::schema::{blocking, Message, PageInfo, ReadService};
use async_graphql::{
    ComplexObject, Context, Enum, InputObject, Interface, Object, Result, SimpleObject, Union, ID,
};
use cwr::v2026_09::conversation::{
    ConversationPublication, ConversationPublicationFilter, ConversationPublicationPage,
    ConversationPublicationSnapshot, ConversationResource, ConversationTimelineChildRole,
    ConversationTimelineCursor, ConversationTimelinePageRequest, ConversationTimelineSourceType,
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(super) struct Cursor(pub String);
#[async_graphql::Scalar]
impl async_graphql::ScalarType for Cursor {
    fn parse(value: async_graphql::Value) -> async_graphql::InputValueResult<Self> {
        match value {
            async_graphql::Value::String(value) => Ok(Self(value)),
            other => Err(async_graphql::InputValueError::expected_type(other)),
        }
    }
    fn to_value(&self) -> async_graphql::Value {
        async_graphql::Value::String(self.0.clone())
    }
}

#[derive(Enum, Clone, Copy, PartialEq, Eq)]
pub(super) enum TimelineSourceType {
    Message,
    ActionInvocation,
}
#[derive(Enum, Clone, Copy, PartialEq, Eq)]
pub(super) enum TimelineChildRole {
    Commentary,
    FinalReply,
}
#[derive(InputObject, Default)]
pub(super) struct TimelineFilter {
    search: Option<String>,
    source_types: Option<Vec<TimelineSourceType>>,
    child_role: Option<TimelineChildRole>,
}
impl TimelineFilter {
    fn normalized(self) -> cwr::CwrResult<ConversationPublicationFilter> {
        ConversationPublicationFilter {
            search: self.search,
            source_types: self
                .source_types
                .unwrap_or_default()
                .into_iter()
                .map(|v| match v {
                    TimelineSourceType::Message => ConversationTimelineSourceType::Message,
                    TimelineSourceType::ActionInvocation => {
                        ConversationTimelineSourceType::ActionInvocation
                    }
                })
                .collect(),
            child_role: self.child_role.map(|role| match role {
                TimelineChildRole::Commentary => ConversationTimelineChildRole::Commentary,
                TimelineChildRole::FinalReply => ConversationTimelineChildRole::FinalReply,
            }),
        }
        .normalized()
    }
}

#[derive(Clone, Default)]
pub(super) struct View(Arc<Mutex<Option<ConversationPublicationSnapshot>>>);
impl View {
    pub fn read<T>(
        &self,
        service: &ReadService,
        read: impl FnOnce(&ConversationPublicationSnapshot) -> cwr::CwrResult<T>,
    ) -> cwr::CwrResult<T> {
        let mut view = loop {
            service.budget().check()?;
            match self.0.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(1))
                }
                Err(_) => {
                    return Err(std::io::Error::other("Publication snapshot lock poisoned").into())
                }
            }
        };
        if view.is_none() {
            let conversations = service
                .runtime
                .bundled_conversation_module_as(&service.ingress, &service.actor_ref)?;
            *view = Some(conversations.publication_snapshot_in_context(
                service.owner.as_deref(),
                service.budget().deadline(),
                || service.budget().cancelled(),
                |read| service.budget().register(read),
            )?);
        }
        read(view.as_ref().expect("initialized snapshot"))
    }
}

#[derive(Clone)]
pub(super) struct TimelineItem {
    service: ReadService,
    value: ConversationPublication,
}
#[Object]
impl TimelineItem {
    async fn id(&self) -> ID {
        ID(self.value.item.item_ref.clone())
    }
    async fn publication_ref(&self) -> ID {
        ID(self.value.item.publication_ref.clone())
    }
    async fn presentation(&self) -> ContentPresentation {
        ContentPresentation {
            text: self.value.text.clone(),
            service: self.service.clone(),
            message: self.value.source.as_ref().and_then(|s| match &s.resource {
                ConversationResource::Message { message, .. } => Some(message.message_ref.clone()),
                _ => None,
            }),
        }
    }
    #[expect(
        clippy::unused_async,
        reason = "GraphQL object resolvers require async methods"
    )]
    async fn source(&self) -> Result<PublicationSource> {
        Ok(match self.value.source.clone() {
            Some(source) => PublicationSource::Resolved(ResolvedSource {
                node: Box::new(resource_node(source.resource, self.service.clone())?),
            }),
            None => PublicationSource::Unavailable(Box::new(UnavailableSource {
                presentation: ContentPresentation {
                    text: "Content unavailable".into(),
                    service: self.service.clone(),
                    message: None,
                },
            })),
        })
    }
    async fn children(
        &self,
        ctx: &Context<'_>,
        filter: Option<TimelineFilter>,
        first: Option<i32>,
        after: Option<Cursor>,
        last: Option<i32>,
        before: Option<Cursor>,
    ) -> Result<Option<TimelineItemConnection>> {
        let Some(connection) = self.value.item.child_connection_ref.clone() else {
            return Ok(None);
        };
        page(
            self.service.clone(),
            self.value.conversation_ref.clone(),
            Some(connection),
            filter,
            first,
            after,
            last,
            before,
            ctx.look_ahead().field("totalCount").exists(),
        )
        .await
    }
}
#[derive(SimpleObject)]
#[graphql(complex)]
pub(super) struct ContentPresentation {
    text: String,
    #[graphql(skip)]
    service: ReadService,
    #[graphql(skip)]
    message: Option<String>,
}
#[ComplexObject]
impl ContentPresentation {
    async fn attachments(
        &self,
        filter: Option<super::attachments::AttachmentFilter>,
        first: Option<i32>,
        after: Option<Cursor>,
        last: Option<i32>,
        before: Option<Cursor>,
    ) -> Result<super::attachments::AttachmentConnection> {
        match &self.message {
            Some(message) => {
                super::attachments::page(
                    self.service.clone(),
                    message.clone(),
                    filter,
                    first,
                    after,
                    last,
                    before,
                )
                .await
            }
            None => super::attachments::empty(filter, first, after, last, before),
        }
    }
}
#[derive(Union)]
pub(super) enum PublicationSource {
    Resolved(ResolvedSource),
    Unavailable(Box<UnavailableSource>),
}
#[derive(SimpleObject)]
pub(super) struct ResolvedSource {
    node: Box<Node>,
}
#[derive(SimpleObject)]
pub(super) struct UnavailableSource {
    presentation: ContentPresentation,
}
#[derive(SimpleObject)]
pub(super) struct TimelineItemConnection {
    connection_ref: ID,
    revision: String,
    edges: Vec<TimelineItemEdge>,
    page_info: PageInfo,
    total_count: i32,
}
#[derive(SimpleObject)]
struct TimelineItemEdge {
    cursor: Cursor,
    node: TimelineItem,
}

#[derive(Interface)]
#[graphql(field(name = "id", ty = "ID"))]
pub(super) enum Node {
    Runtime(super::schema::Runtime),
    Conversation(super::schema::Conversation),
    Message(Message),
    Coworker(super::messages::Coworker),
    UnresolvedActor(super::messages::UnresolvedActor),
    StreamingMessage(StreamingMessage),
    HtmlVisualization(super::attachments::HtmlVisualization),
    TimelineItem(TimelineItem),
    Admitted(super::invocations::AdmittedInvocation),
    AwaitingPerson(super::invocations::AwaitingPersonInvocation),
    Running(super::invocations::RunningInvocation),
    Completed(super::invocations::CompletedInvocation),
    Blocked(super::invocations::BlockedInvocation),
    Failed(super::invocations::FailedInvocation),
    Stopping(super::invocations::StoppingInvocation),
    Stopped(super::invocations::StoppedInvocation),
}
pub(super) fn resource_node(
    resource: ConversationResource,
    service: ReadService,
) -> cwr::CwrResult<Node> {
    Ok(match resource {
        ConversationResource::Message {
            message, streaming, ..
        } => {
            let value = Message {
                service,
                id: ID(message.message_ref),
                author_actor_ref: ID(message.author_actor_ref),
                addressed_actor_refs: message.addressed_actor_refs.into_iter().map(ID).collect(),
                reply_to_message_ref: message.reply_to_message_ref.map(ID),
                text: message.text,
            };
            if streaming {
                Node::StreamingMessage(StreamingMessage(value))
            } else {
                Node::Message(value)
            }
        }
        ConversationResource::Invocation { invocation } => {
            match super::invocations::project(invocation, service)? {
                super::invocations::ActionInvocation::Admitted(v) => Node::Admitted(v),
                super::invocations::ActionInvocation::AwaitingPerson(v) => Node::AwaitingPerson(v),
                super::invocations::ActionInvocation::Running(v) => Node::Running(v),
                super::invocations::ActionInvocation::Completed(v) => Node::Completed(v),
                super::invocations::ActionInvocation::Blocked(v) => Node::Blocked(v),
                super::invocations::ActionInvocation::Failed(v) => Node::Failed(v),
                super::invocations::ActionInvocation::Stopping(v) => Node::Stopping(v),
                super::invocations::ActionInvocation::Stopped(v) => Node::Stopped(v),
            }
        }
    })
}
fn cursor(
    service: &ReadService,
    filter: &ConversationPublicationFilter,
    value: &ConversationTimelineCursor,
) -> cwr::CwrResult<String> {
    Ok(serde_json::to_string(&(
        &service.actor_ref,
        &service.owner,
        filter,
        value,
    ))?)
}
fn decode(
    service: &ReadService,
    filter: &ConversationPublicationFilter,
    value: String,
) -> Result<ConversationTimelineCursor> {
    let (actor, owner, scope, cursor): (
        String,
        Option<String>,
        ConversationPublicationFilter,
        ConversationTimelineCursor,
    ) = serde_json::from_str(&value)?;
    if scope != *filter {
        return Err("Invalid cursor filter scope".into());
    }
    if actor != service.actor_ref || owner != service.owner {
        return Err("Invalid cursor authorization scope".into());
    }
    Ok(cursor)
}
fn connection(
    service: ReadService,
    value: ConversationPublicationPage,
    filter: &ConversationPublicationFilter,
) -> cwr::CwrResult<TimelineItemConnection> {
    let start = value
        .page
        .page_info
        .start_cursor
        .as_ref()
        .map(|v| cursor(&service, filter, v))
        .transpose()?;
    let end = value
        .page
        .page_info
        .end_cursor
        .as_ref()
        .map(|v| cursor(&service, filter, v))
        .transpose()?;
    let edges = value
        .page
        .edges
        .into_iter()
        .zip(value.publications)
        .map(|(edge, publication)| {
            Ok(TimelineItemEdge {
                cursor: Cursor(cursor(&service, filter, &edge.cursor)?),
                node: TimelineItem {
                    service: service.clone(),
                    value: publication,
                },
            })
        })
        .collect::<cwr::CwrResult<Vec<_>>>()?;
    let total_count = i32::try_from(value.total_count.unwrap_or_default())
        .map_err(|_| std::io::Error::other("Exact count exceeds GraphQL Int"))?;
    Ok(TimelineItemConnection {
        connection_ref: ID(value.page.connection_ref),
        revision: value.revision,
        edges,
        page_info: PageInfo::new(
            value.page.page_info.has_next_page,
            value.page.page_info.has_previous_page,
            start,
            end,
        ),
        total_count,
    })
}
#[expect(
    clippy::too_many_arguments,
    reason = "Relay pagination passes four independent window arguments"
)]
pub(super) async fn page(
    service: ReadService,
    conversation: String,
    connection_ref: Option<String>,
    filter: Option<TimelineFilter>,
    first: Option<i32>,
    after: Option<Cursor>,
    last: Option<i32>,
    before: Option<Cursor>,
    count: bool,
) -> Result<Option<TimelineItemConnection>> {
    let filter = filter.unwrap_or_default().normalized()?;
    let size = |v: i32| -> Result<u32> {
        if !(1..=100).contains(&v) {
            Err("Page size must be between 1 and 100".into())
        } else {
            Ok(v as u32)
        }
    };
    let first = first.map(size).transpose()?;
    let last = last.map(size).transpose()?;
    let request = ConversationTimelinePageRequest {
        first: if first.is_none() && last.is_none() {
            Some(50)
        } else {
            first
        },
        last,
        after: after.map(|v| decode(&service, &filter, v.0)).transpose()?,
        before: before.map(|v| decode(&service, &filter, v.0)).transpose()?,
    };
    blocking(service.reads.clone(), service.budget(), move || {
        service.catalog()?;
        let value = service.publications.read(&service, |view| {
            view.filtered_page(
                &conversation,
                connection_ref.as_deref(),
                &request,
                &filter,
                count,
            )
        })?;
        value
            .map(|value| connection(service.clone(), value, &filter))
            .transpose()
    })
    .await
}
pub(super) async fn lookup_item(
    service: ReadService,
    reference: String,
) -> Result<Option<TimelineItem>> {
    blocking(service.reads.clone(), service.budget(), move || {
        service.catalog()?;
        Ok(service
            .publications
            .read(&service, |view| view.item(&reference))?
            .map(|value| TimelineItem {
                service: service.clone(),
                value,
            }))
    })
    .await
}
pub(super) async fn lookup_node(service: ReadService, reference: String) -> Result<Option<Node>> {
    blocking(service.reads.clone(), service.budget(), move || {
        service.catalog()?;
        if reference == service.runtime_ref {
            return Ok(Some(Node::Runtime(super::schema::Runtime(service.clone()))));
        }
        if let Some(value) = service
            .publications
            .read(&service, |view| view.item(&reference))?
        {
            return Ok(Some(Node::TimelineItem(TimelineItem {
                service: service.clone(),
                value,
            })));
        }
        if let Some(value) = service
            .publications
            .read(&service, |view| view.resource(&reference))?
        {
            return resource_node(value.resource, service.clone()).map(Some);
        }
        if let Some((title, archived)) = service
            .publications
            .read(&service, |view| view.conversation(&reference))?
        {
            return Ok(Some(Node::Conversation(
                super::schema::Conversation::from_snapshot(
                    service.clone(),
                    reference.clone(),
                    title,
                    archived,
                ),
            )));
        }
        if let Some(value) = service
            .publications
            .read(&service, |view| view.attachment(&reference))?
        {
            return Ok(Some(Node::HtmlVisualization(
                super::attachments::HtmlVisualization::new(service.clone(), value),
            )));
        }
        if let Some(value) = service
            .publications
            .read(&service, |view| view.actor(&reference))?
        {
            return Ok(Some(match super::messages::actor(value) {
                super::messages::Actor::Coworker(value) => Node::Coworker(value),
                super::messages::Actor::Unresolved(value) => Node::UnresolvedActor(value),
            }));
        }
        Ok(None)
    })
    .await
}

pub(super) use super::messages::{lookup_message, MessageInterface, StreamingMessage};
