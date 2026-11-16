use super::*;

impl ConversationAdmissionStore {
    pub(crate) fn published_progress_conversation_ref(
        &self,
        source_activity_ref: &str,
    ) -> CwrResult<Option<String>> {
        let mut backend = self
            .backend
            .lock()
            .map_err(|_| invalid_error("conversation backend lock poisoned"))?;
        backend.progress_conversation(source_activity_ref)
    }
}

pub(super) fn source_message_ref(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<String> {
    let execution = invocation::execution_conversation_in_state(state, invocation_ref)?;
    execution
        .activity
        .iter()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::TurnStarted {
                invocation_ref: started_invocation_ref,
                source_message_ref,
                ..
            } if started_invocation_ref == invocation_ref => Some(source_message_ref.clone()),
            _ => None,
        })
        .ok_or_else(|| invalid_error("invocation has no source message"))
}

pub(super) fn history_gap_source_message_ref(
    state: &ConversationRuntimeState,
    thread: &Value,
) -> CwrResult<Option<String>> {
    let working = &thread["working_conversation"];
    if !working["history_gap"].is_string() {
        return Ok(None);
    }
    let previous_invocation_ref = working["previous_invocation_ref"]
        .as_str()
        .ok_or("private history gap has no previous invocation")?;
    source_message_ref(state, previous_invocation_ref).map(Some)
}

pub(super) fn publish(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    activity: &ConversationActivity,
    text: String,
    status: Option<ActionResponseStatus>,
) -> CwrResult<()> {
    let (conversation_ref, source, actor_ref) = public_destination(state, invocation_ref)?;
    let message_ref =
        super::super::streaming::finalize_commentary(state, invocation_ref, activity)?
            .unwrap_or_else(|| new_ref(&state.runtime_root_ref, "messages"));
    let conversation = state
        .conversations
        .find_mut(|c| c.conversation_ref == conversation_ref)
        .ok_or("progress conversation is unavailable")?;
    if conversation.messages.iter().any(|message| {
        message
            .progress
            .as_ref()
            .is_some_and(|progress| progress.source_activity_ref == activity.activity_ref)
    }) {
        return Ok(());
    }
    conversation.record_message(ConversationMessage {
        file_refs: Vec::new(),
        message_ref,
        author_actor_ref: actor_ref,
        addressed_actor_refs: Vec::new(),
        addressing_mode: Some(ConversationAddressingMode::Implicit),
        text,
        visualizations: Vec::new(),
        reply_to_message_ref: source,
        action: None,
        progress: Some(Box::new(ConversationProgress {
            invocation_ref: invocation_ref.into(),
            source_activity_ref: activity.activity_ref.clone(),
            status,
        })),
    });
    Ok(())
}

fn public_author_actor_ref(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<String> {
    let record = state
        .invocation_ledger
        .invocation(invocation_ref)
        .ok_or("progress has no invocation")?;
    if record.parent_invocation_ref.is_none() {
        return Ok(record.target_actor_ref.clone());
    }
    let source_message_ref = source_message_ref(state, invocation_ref)?;
    let source = state.conversations.iter().find_map(|conversation| {
        conversation
            .messages
            .iter()
            .find(|message| message.message_ref == source_message_ref)
    });
    let hidden_gateway_target = source.is_some_and(|message| {
        message.author_actor_ref == record.requester_actor_ref
            && !message
                .addressed_actor_refs
                .contains(&record.target_actor_ref)
    });
    Ok(if hidden_gateway_target {
        record.requester_actor_ref.clone()
    } else {
        record.target_actor_ref.clone()
    })
}

/// Public progress belongs to the shared conversation that admitted the root
/// invocation. The emitting invocation remains its origin, while the reply
/// points at the root request rather than a private child call. A gateway's
/// hidden transport actor is represented by its user-visible caller.
pub(super) fn public_destination(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<(String, Option<String>, String)> {
    let actor_ref = public_author_actor_ref(state, invocation_ref)?;
    let mut root_invocation_ref = invocation_ref.to_owned();
    let mut remaining = state.invocation_ledger.invocations().count() + 1;
    loop {
        if remaining == 0 {
            return invalid("progress invocation ancestry contains a cycle");
        }
        remaining -= 1;
        let record = state
            .invocation_ledger
            .invocation(&root_invocation_ref)
            .ok_or("progress invocation ancestry is unavailable")?;
        let Some(parent_invocation_ref) = &record.parent_invocation_ref else {
            let conversation_ref = record
                .conversation_ref
                .clone()
                .ok_or("progress has no shared conversation")?;
            let conversation = state
                .conversations
                .iter()
                .find(|conversation| conversation.conversation_ref == conversation_ref)
                .ok_or("progress shared conversation is unavailable")?;
            if conversation.parent_conversation_ref.is_some() {
                return invalid("progress invocation root is not in a shared conversation");
            }
            let execution =
                invocation::execution_conversation_in_state(state, &root_invocation_ref)?;
            let source = execution
                .activity
                .iter()
                .find_map(|activity| match &activity.event {
                    ConversationActivityEvent::TurnStarted {
                        invocation_ref: started_invocation_ref,
                        source_message_ref,
                        ..
                    } if started_invocation_ref == &root_invocation_ref => {
                        Some(source_message_ref.clone())
                    }
                    _ => None,
                });
            if source.as_ref().is_some_and(|source| {
                !conversation
                    .messages
                    .iter()
                    .any(|message| &message.message_ref == source)
            }) {
                return invalid("progress root source is unavailable in the shared conversation");
            }
            return Ok((conversation_ref, source, actor_ref));
        };
        root_invocation_ref = parent_invocation_ref.clone();
    }
}
