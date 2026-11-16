//! Message authorship and host-local routing come only from the authenticated session.
use super::{mutations::read_invocation, schema::ReadService};
use async_graphql::{Context, Error, ErrorExtensions, Object, Result, SimpleObject, ID};
#[derive(Default)]
pub(super) struct MessageMutations;
#[derive(SimpleObject)]
struct MessageAdmission {
    conversation_ref: ID,
    conversation: super::schema::Conversation,
    message_ref: ID,
    invocation: Option<super::mutations::ActionInvocation>,
}
#[Object]
impl MessageMutations {
    /// Append to an existing conversation. Its owner cannot be changed by a send.
    async fn send_message(
        &self,
        ctx: &Context<'_>,
        text: String,
        request_id: ID,
        conversation_ref: ID,
        file_refs: Option<Vec<ID>>,
    ) -> Result<MessageAdmission> {
        if conversation_ref.trim().is_empty() {
            return Err("conversationRef must not be empty".into());
        }
        admit_message(
            ctx,
            text,
            request_id,
            Some(conversation_ref),
            file_refs,
            None,
        )
        .await
    }

    /// Create a conversation in the selected pack or Workspace context.
    /// Omitting the owner selects the runtime's default Workspace context.
    async fn create_conversation(
        &self,
        ctx: &Context<'_>,
        text: String,
        request_id: ID,
        owner_context_ref: Option<ID>,
        file_refs: Option<Vec<ID>>,
    ) -> Result<MessageAdmission> {
        if owner_context_ref
            .as_ref()
            .is_some_and(|owner| owner.trim().is_empty())
        {
            return Err("ownerContextRef must not be empty".into());
        }
        admit_message(ctx, text, request_id, None, file_refs, owner_context_ref).await
    }
}

async fn admit_message(
    ctx: &Context<'_>,
    text: String,
    request_id: ID,
    conversation_ref: Option<ID>,
    file_refs: Option<Vec<ID>>,
    owner_context_ref: Option<ID>,
) -> Result<MessageAdmission> {
    let file_refs = file_refs
        .unwrap_or_default()
        .into_iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    if (text.trim().is_empty() && file_refs.is_empty())
        || request_id.is_empty()
        || request_id.len() > 256
    {
        return Err(
            "text and requestId must not be empty; requestId must not exceed 256 bytes".into(),
        );
    }
    let mut service = ctx.data_unchecked::<ReadService>().clone();
    // Earlier mutation fields may already have opened the operation's read view.
    service.publications = Default::default();
    tokio::task::spawn_blocking(move || {
        let key = serde_json::to_string(&(&service.actor_ref, request_id.as_str()))
            .map_err(|_| Error::new("Unable to encode request identity"))?;
        let unknown = || {
            Error::new(
                "Message outcome could not be confirmed; retry with the same requestId and input",
            )
            .extend_with(|_, e| e.set("code", "MESSAGE_OUTCOME_UNKNOWN"))
        };
        let module = service
            .runtime
            .bundled_conversation_module_as(&service.ingress, &service.actor_ref)
            .map_err(|_| unknown())?;
        let admission = module
            .publish_message_in_context(
                &key,
                conversation_ref.as_ref().map(|id| id.as_str()),
                text,
                file_refs,
                service.owner.as_deref(),
                owner_context_ref.as_ref().map(|id| id.as_str()),
            )
            .map_err(|error| {
                if error
                    .downcast_ref::<cwr::v2026_09::conversation::ConversationContextError>()
                    .is_some()
                {
                    Error::new(error.to_string()).extend_with(|_, extensions| {
                        extensions.set("code", "CONVERSATION_CONTEXT_REJECTED")
                    })
                } else {
                    unknown()
                }
            })?;
        (service.publisher)(&admission).map_err(|_| unknown())?;
        let invocation = admission
            .invocation_ref()
            .map(|id| read_invocation(&service, id))
            .transpose()
            .map_err(|_| unknown())?
            .flatten();
        let conversation_ref = admission.conversation_ref().to_string();
        let (title, archived) = service
            .publications
            .read(&service, |view| view.conversation(&conversation_ref))
            .map_err(|_| unknown())?
            .ok_or_else(unknown)?;
        let conversation = super::schema::Conversation::from_snapshot(
            service.clone(),
            conversation_ref,
            title,
            archived,
        );
        Ok(MessageAdmission {
            conversation,
            conversation_ref: ID(admission.conversation_ref().into()),
            message_ref: ID(admission.record.source_message_ref),
            invocation,
        })
    })
    .await?
}
