pub(super) fn admission_state(response: &ActionResponse) -> CwrResult<ConversationAdmissionState> {
    if response.status.is_completed() {
        Ok(ConversationAdmissionState::Completed)
    } else if response.status.is_stopped() {
        Ok(ConversationAdmissionState::Stopped)
    } else if response.status.is_blocked() || response.status.is_failed() {
        Ok(ConversationAdmissionState::Failed)
    } else {
        invalid("progress cannot terminalize an admission")
    }
}

pub(super) fn terminal_activity(response: &ActionResponse) -> CwrResult<ConversationActivityEvent> {
    if response.status.is_completed() {
        Ok(ConversationActivityEvent::TurnCompleted {
            status: response.status,
            summary: response.summary.clone(),
            result: response.result.clone(),
        })
    } else if response.status.is_stopped() {
        Ok(ConversationActivityEvent::TurnStopped {
            status: response.status,
            reason: response
                .cause
                .as_ref()
                .and_then(|cause| cause.message.clone()),
        })
    } else if response.status.is_blocked() || response.status.is_failed() {
        Ok(ConversationActivityEvent::TurnFailed {
            status: response.status,
            error: response
                .error
                .as_ref()
                .and_then(|error| error.message.clone())
                .unwrap_or_else(|| response.summary.clone()),
        })
    } else {
        invalid("progress has no terminal activity")
    }
}

fn terminal_reply_in_state(
    state: &ConversationRuntimeState,
    admission_index: usize,
) -> CwrResult<ConversationMessage> {
    let record = &state.admissions[admission_index];
    let reply_ref = record
        .reply_message_ref
        .as_deref()
        .ok_or_else(|| invalid_error("terminal admission has no reply message ref"))?;
    state
        .conversations
        .iter()
        .find(|conversation| conversation.conversation_ref == record.conversation_ref)
        .and_then(|conversation| {
            conversation
                .messages
                .iter()
                .find(|message| message.message_ref == reply_ref)
        })
        .cloned()
        .ok_or_else(|| invalid_error("terminal admission reply message is unavailable"))
}

fn existing_root_reply(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<Option<ConversationMessage>> {
    let admission_index = root_admission_index(state, invocation_ref)?;
    if state.admissions[admission_index]
        .reply_message_ref
        .is_none()
    {
        return Ok(None);
    }
    terminal_reply_in_state(state, admission_index).map(Some)
}

fn existing_child_result(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<Option<ConversationMessage>> {
    let invocation = state
        .invocation_ledger
        .invocation(invocation_ref)
        .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
    let conversation_ref = invocation
        .conversation_ref
        .as_deref()
        .ok_or_else(|| invalid_error("child invocation has no conversation ref"))?;
    let conversation = state.conversations.iter()
        .find(|conversation| conversation.conversation_ref == conversation_ref);
    if let Some(reply_ref) = &invocation.reply_message_ref {
        return Ok(conversation.and_then(|conversation| conversation.messages.iter()
            .find(|message| &message.message_ref == reply_ref)).cloned());
    }
    // Older records have no publication link. Never guess between invocations
    // sharing a source and target, or between multiple possible messages.
    let source_message_ref = invocation_source_message_ref(state, invocation_ref)?;
    for other in state.invocation_ledger.invocations() {
        if other.invocation_ref != invocation_ref
            && other.conversation_ref.as_deref() == Some(conversation_ref)
            && other.target_actor_ref == invocation.target_actor_ref
            && invocation_source_message_ref(state, &other.invocation_ref).ok().as_deref()
                == Some(source_message_ref.as_str())
        {
            return Ok(None);
        }
    }
    let mut candidates = conversation.into_iter().flat_map(|conversation| conversation.messages.iter())
        .filter(|message| {
            message.progress.is_none()
                && message.author_actor_ref == invocation.target_actor_ref
                && message.reply_to_message_ref.as_deref() == Some(source_message_ref.as_str())
                && (invocation.action_name == RESPOND_TO_MESSAGE_ACTION_NAME
                    || matches!(&message.action, Some(ConversationActionEvent::Result { .. })))
        });
    let first = candidates.next();
    Ok(if candidates.next().is_none() { first.cloned() } else { None })
}

fn existing_legacy_reply(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
    expected_state: ConversationAdmissionState,
) -> CwrResult<Option<ConversationMessage>> {
    let admission_index = root_admission_index(state, invocation_ref)?;
    let current = state.admissions[admission_index].state;
    if current == expected_state {
        return terminal_reply_in_state(state, admission_index).map(Some);
    }
    if current != ConversationAdmissionState::Admitted {
        return invalid(format!(
            "invocation {invocation_ref} is not awaiting completion"
        ));
    }
    Ok(None)
}
