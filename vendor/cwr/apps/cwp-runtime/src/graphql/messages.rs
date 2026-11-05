//! Public metadata projection; source reads share the selection's frozen view.
use super::{
    publications::{lookup_node, Cursor, Node},
    schema::{blocking, Message, ReadService},
};
use async_graphql::{ComplexObject, Context, Interface, Object, Result, SimpleObject, ID};
use cwr::v2026_09::conversation::{
    ConversationActionEvent, ConversationActorPresentation, ConversationMessageMetadata,
    ConversationResource,
};

#[derive(Clone)]
pub(super) struct DateTime(String);
#[async_graphql::Scalar]
impl async_graphql::ScalarType for DateTime {
    fn parse(value: async_graphql::Value) -> async_graphql::InputValueResult<Self> {
        match value {
            async_graphql::Value::String(s) => Ok(Self(s)),
            other => Err(async_graphql::InputValueError::expected_type(other)),
        }
    }
    fn to_value(&self) -> async_graphql::Value {
        self.0.clone().into()
    }
}
pub(super) fn datetime(ms: i128) -> cwr::CwrResult<DateTime> {
    let t = time::OffsetDateTime::from_unix_timestamp_nanos(ms * 1_000_000)?;
    Ok(DateTime(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )))
}
pub(super) async fn metadata(message: &Message) -> Result<Option<ConversationMessageMetadata>> {
    let service = message.service.clone();
    let reference = message.id.to_string();
    blocking(service.reads.clone(), service.budget(), move || {
        service
            .publications
            .read(&service, |view| view.message_metadata(&reference))
    })
    .await
}
pub(super) async fn invocation(
    service: &ReadService,
    reference: Option<&str>,
) -> Result<Option<super::invocations::ActionInvocation>> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    let service = service.clone();
    let reference = reference.to_owned();
    blocking(
        service.reads.clone(),
        service.budget(),
        move || match service
            .publications
            .read(&service, |view| view.resource(&reference))?
            .map(|v| v.resource)
        {
            Some(ConversationResource::Invocation { invocation }) => {
                super::invocations::project(invocation, service.clone()).map(Some)
            }
            _ => Ok(None),
        },
    )
    .await
}
#[derive(Clone, SimpleObject)]
#[graphql(complex)]
pub(super) struct Coworker {
    #[graphql(skip)]
    pub(super) configuration: std::sync::Arc<
        tokio::sync::OnceCell<Option<cwr::v2026_09::local::LocalCoworkerConfiguration>>,
    >,
    #[graphql(owned)]
    pub(super) id: ID,
    #[graphql(owned)]
    name: Option<String>,
    handle: Option<String>,
}
impl From<cwr::v2026_09::local::LocalActorIdentity> for Coworker {
    fn from(actor: cwr::v2026_09::local::LocalActorIdentity) -> Self {
        Self {
            configuration: Default::default(),
            id: ID(actor.actor_ref),
            name: Some(actor.name),
            handle: Some(actor.handle),
        }
    }
}
#[derive(Clone, SimpleObject)]
pub(super) struct UnresolvedActor {
    #[graphql(owned)]
    pub(super) id: ID,
    #[graphql(owned)]
    name: Option<String>,
}
#[derive(Clone, Interface)]
#[graphql(
    field(name = "id", ty = "ID"),
    field(name = "name", ty = "Option<String>")
)]
pub(super) enum Actor {
    Coworker(Coworker),
    Unresolved(UnresolvedActor),
}
pub(super) fn actor(value: ConversationActorPresentation) -> Actor {
    if value.coworker {
        Actor::Coworker(Coworker {
            configuration: Default::default(),
            id: ID(value.actor_ref),
            name: value.name,
            handle: value.handle,
        })
    } else {
        Actor::Unresolved(UnresolvedActor {
            id: ID(value.actor_ref),
            name: value.name,
        })
    }
}
#[derive(Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub(super) enum AddressingMode {
    Explicit,
    Implicit,
}
#[ComplexObject]
impl Message {
    /// How the retained message addressed its recipients; unknown records remain null.
    pub(super) async fn addressing_mode(&self) -> Result<Option<AddressingMode>> {
        use cwr::v2026_09::conversation::ConversationAddressingMode;
        Ok(metadata(self)
            .await?
            .and_then(|m| m.message.addressing_mode)
            .map(|m| match m {
                ConversationAddressingMode::Explicit => AddressingMode::Explicit,
                ConversationAddressingMode::Implicit => AddressingMode::Implicit,
            }))
    }

    pub(super) async fn recipients(
        &self,
        filter: Option<super::actors::ActorFilter>,
        first: Option<i32>,
        after: Option<Cursor>,
        last: Option<i32>,
        before: Option<Cursor>,
    ) -> Result<super::actors::ActorConnection> {
        super::actors::recipients(
            self.service.clone(),
            self.id.to_string(),
            filter,
            first,
            after,
            last,
            before,
        )
        .await
    }
    pub(super) async fn files(&self) -> Result<Vec<super::files::FileMetadata>> {
        let refs = metadata(self)
            .await?
            .map(|m| m.message.file_refs.clone())
            .unwrap_or_default();
        let mut files = Vec::new();
        for reference in refs {
            files.push(super::files::metadata(self.service.clone(), reference).await?);
        }
        Ok(files)
    }
    pub(super) async fn attachments(
        &self,
        filter: Option<super::attachments::AttachmentFilter>,
        first: Option<i32>,
        after: Option<super::publications::Cursor>,
        last: Option<i32>,
        before: Option<super::publications::Cursor>,
    ) -> Result<super::attachments::AttachmentConnection> {
        super::attachments::page(
            self.service.clone(),
            self.id.to_string(),
            filter,
            first,
            after,
            last,
            before,
        )
        .await
    }
    pub(super) async fn occurred_at(&self) -> Result<Option<DateTime>> {
        Ok(metadata(self)
            .await?
            .and_then(|m| m.occurred_at_unix_ms)
            .map(|ms| datetime(i128::from(ms)))
            .transpose()?)
    }
    pub(super) async fn reply_to(&self) -> Result<Option<MessageInterface>> {
        match &self.reply_to_message_ref {
            Some(reference) => lookup_message(self.service.clone(), reference.to_string()).await,
            None => Ok(None),
        }
    }
    pub(super) async fn author(&self) -> Result<Actor> {
        let service = self.service.clone();
        let reference = self.id.to_string();
        let author = self.author_actor_ref.to_string();
        blocking(service.reads.clone(), service.budget(), move || {
            let value = service
                .publications
                .read(&service, |view| view.related_actor(&reference, &author))?
                .ok_or_else(|| std::io::Error::other("Message author unavailable"))?;
            Ok(actor(value))
        })
        .await
    }
    pub(super) async fn progress(&self) -> Result<Option<MessageProgress>> {
        let Some(value) = metadata(self).await? else {
            return Ok(None);
        };
        let timing = value
            .progress_timing()?
            .map(super::timing::project)
            .transpose()?;
        let Some(progress) = value.message.progress else {
            return Ok(None);
        };
        let Some(invocation) = invocation(&self.service, value.invocation_ref.as_deref()).await?
        else {
            return Ok(None);
        };
        Ok(Some(MessageProgress {
            timing,
            response: value
                .progress_response_status
                .zip(value.progress_summary)
                .map(|(status, summary)| informational(status, summary)),
            invocation,
            source_activity_ref: ID(progress.source_activity_ref),
            status: progress.status.map(|s| i32::from(s.as_u16())),
            occurred_at: value
                .progress_occurred_at_unix_ms
                .map(|ms| datetime(i128::from(ms)))
                .transpose()?,
        }))
    }
    pub(super) async fn visualizations(
        &self,
    ) -> Result<Vec<super::rich_messages::MessageVisualization>> {
        super::rich_messages::visualizations(self).await
    }
    pub(super) async fn action(&self) -> Result<Option<MessageAction>> {
        let Some(value) = metadata(self).await? else {
            return Ok(None);
        };
        let linked = invocation(&self.service, value.invocation_ref.as_deref()).await?;
        Ok(match value.message.action {
            Some(ConversationActionEvent::Call { activation, input }) => {
                let title = self.service.action_title(&activation).await?;
                Some(MessageAction::Call(MessageActionCall {
                    invocation: linked,
                    title,
                    selector: activation,
                    input: async_graphql::Json(input),
                }))
            }
            Some(ConversationActionEvent::Result { result, .. }) => {
                value.action_status.map(|status| {
                    MessageAction::Result(MessageActionResult {
                        invocation: linked,
                        title: None,
                        status: i32::from(status),
                        summary: value.action_summary,
                        result: async_graphql::Json(result),
                    })
                })
            }
            None => None,
        })
    }
}
#[derive(Clone, SimpleObject)]
pub(super) struct MessageProgress {
    timing: Option<super::timing::ActivityTiming>,
    response: Option<InformationalOutcome>,
    invocation: super::invocations::ActionInvocation,
    source_activity_ref: ID,
    status: Option<i32>,
    occurred_at: Option<DateTime>,
}
#[derive(Clone, SimpleObject)]
pub(super) struct MessageActionCall {
    #[graphql(owned)]
    invocation: Option<super::invocations::ActionInvocation>,
    #[graphql(owned)]
    title: Option<String>,
    selector: String,
    input: async_graphql::Json<serde_json::Value>,
}
#[derive(Clone, SimpleObject)]
pub(super) struct MessageActionResult {
    #[graphql(owned)]
    invocation: Option<super::invocations::ActionInvocation>,
    #[graphql(owned)]
    title: Option<String>,
    status: i32,
    summary: Option<String>,
    result: async_graphql::Json<serde_json::Value>,
}
#[derive(Clone, Interface)]
#[graphql(
    field(
        name = "invocation",
        ty = "Option<super::invocations::ActionInvocation>"
    ),
    field(name = "title", ty = "Option<String>")
)]
pub(super) enum MessageAction {
    Call(MessageActionCall),
    Result(MessageActionResult),
}

#[derive(Clone)]
pub(super) struct StreamingMessage(pub(super) Message);
#[Object]
impl StreamingMessage {
    async fn visualizations(&self) -> Result<Vec<super::rich_messages::MessageVisualization>> {
        super::rich_messages::visualizations(&self.0).await
    }
    async fn phase(&self) -> Result<String> {
        metadata(&self.0)
            .await?
            .and_then(|value| value.streaming_phase)
            .ok_or_else(|| "Streaming phase unavailable".into())
    }

    pub(super) async fn id(&self) -> ID {
        self.0.id.clone()
    }
    async fn text(&self) -> String {
        self.0.text.clone()
    }
    async fn author_actor_ref(&self) -> ID {
        self.0.author_actor_ref.clone()
    }
    async fn addressed_actor_refs(&self) -> Vec<ID> {
        self.0.addressed_actor_refs.clone()
    }
    async fn reply_to_message_ref(&self) -> Option<ID> {
        self.0.reply_to_message_ref.clone()
    }
    async fn recipients(
        &self,
        ctx: &Context<'_>,
        filter: Option<super::actors::ActorFilter>,
        first: Option<i32>,
        after: Option<Cursor>,
        last: Option<i32>,
        before: Option<Cursor>,
    ) -> Result<super::actors::ActorConnection> {
        self.0
            .recipients(ctx, filter, first, after, last, before)
            .await
    }
    async fn files(&self, ctx: &Context<'_>) -> Result<Vec<super::files::FileMetadata>> {
        self.0.files(ctx).await
    }
    async fn attachments(
        &self,
        ctx: &Context<'_>,
        filter: Option<super::attachments::AttachmentFilter>,
        first: Option<i32>,
        after: Option<Cursor>,
        last: Option<i32>,
        before: Option<Cursor>,
    ) -> Result<super::attachments::AttachmentConnection> {
        self.0
            .attachments(ctx, filter, first, after, last, before)
            .await
    }
    async fn addressing_mode(&self, ctx: &Context<'_>) -> Result<Option<AddressingMode>> {
        self.0.addressing_mode(ctx).await
    }
    async fn occurred_at(&self, ctx: &Context<'_>) -> Result<Option<super::messages::DateTime>> {
        self.0.occurred_at(ctx).await
    }
    async fn reply_to(&self, ctx: &Context<'_>) -> Result<Option<MessageInterface>> {
        self.0.reply_to(ctx).await
    }
    async fn progress(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<super::messages::MessageProgress>> {
        self.0.progress(ctx).await
    }
    async fn action(&self, ctx: &Context<'_>) -> Result<Option<super::messages::MessageAction>> {
        self.0.action(ctx).await
    }
    async fn author(&self, ctx: &Context<'_>) -> Result<super::messages::Actor> {
        self.0.author(ctx).await
    }
    async fn invocation(&self) -> Result<super::invocations::ActionInvocation> {
        let metadata = super::messages::metadata(&self.0)
            .await?
            .ok_or("Streaming message unavailable")?;
        super::messages::invocation(&self.0.service, metadata.invocation_ref.as_deref())
            .await?
            .ok_or_else(|| "Streaming producer unavailable".into())
    }
}
#[derive(Interface)]
#[expect(
    clippy::duplicated_attributes,
    reason = "GraphQL interface fields share types"
)]
#[graphql(
    name = "Message",
    field(name = "id", ty = "ID"),
    field(name = "text", ty = "String"),
    field(name = "files", ty = "Vec<super::files::FileMetadata>"),
    field(name = "author_actor_ref", ty = "ID"),
    field(name = "addressed_actor_refs", ty = "Vec<ID>"),
    field(name = "reply_to_message_ref", ty = "Option<ID>"),
    field(name = "addressing_mode", ty = "Option<AddressingMode>"),
    field(name = "occurred_at", ty = "Option<super::messages::DateTime>"),
    field(name = "reply_to", ty = "Option<MessageInterface>"),
    field(name = "progress", ty = "Option<super::messages::MessageProgress>"),
    field(name = "action", ty = "Option<super::messages::MessageAction>"),
    field(name = "author", ty = "super::messages::Actor"),
    field(
        name = "recipients",
        ty = "super::actors::ActorConnection",
        arg(name = "filter", ty = "Option<super::actors::ActorFilter>"),
        arg(name = "first", ty = "Option<i32>"),
        arg(name = "after", ty = "Option<Cursor>"),
        arg(name = "last", ty = "Option<i32>"),
        arg(name = "before", ty = "Option<Cursor>")
    ),
    field(
        name = "attachments",
        ty = "super::attachments::AttachmentConnection",
        arg(name = "filter", ty = "Option<super::attachments::AttachmentFilter>"),
        arg(name = "first", ty = "Option<i32>"),
        arg(name = "after", ty = "Option<Cursor>"),
        arg(name = "last", ty = "Option<i32>"),
        arg(name = "before", ty = "Option<Cursor>")
    )
)]
pub(super) enum MessageInterface {
    Published(Message),
    Streaming(StreamingMessage),
}
pub(super) async fn lookup_message(
    service: ReadService,
    reference: String,
) -> Result<Option<MessageInterface>> {
    Ok(match lookup_node(service, reference).await? {
        Some(Node::Message(message)) => Some(MessageInterface::Published(message)),
        Some(Node::StreamingMessage(message)) => Some(MessageInterface::Streaming(message)),
        _ => None,
    })
}

#[derive(Clone, SimpleObject)]
pub(super) struct ProgressOutcome {
    #[graphql(owned)]
    status: i32,
    #[graphql(owned)]
    summary: String,
}
#[derive(Clone, SimpleObject)]
pub(super) struct OtherInformationalOutcome {
    #[graphql(owned)]
    status: i32,
    #[graphql(owned)]
    summary: String,
}
#[derive(Clone, Interface)]
#[graphql(
    field(name = "status", ty = "i32"),
    field(name = "summary", ty = "String")
)]
pub(super) enum InformationalOutcome {
    Progress(ProgressOutcome),
    Other(OtherInformationalOutcome),
}
fn informational(status: u16, summary: String) -> InformationalOutcome {
    if status == 102 {
        InformationalOutcome::Progress(ProgressOutcome {
            status: i32::from(status),
            summary,
        })
    } else {
        InformationalOutcome::Other(OtherInformationalOutcome {
            status: i32::from(status),
            summary,
        })
    }
}
