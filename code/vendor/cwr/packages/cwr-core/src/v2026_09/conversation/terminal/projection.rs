fn stage_root_projection(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    response: &ActionResponse,
    text: String,
    reply_kind: RootReplyKind,
    activity: Option<(ConversationActivityEvent, u64)>,
) -> CwrResult<Option<ConversationMessage>> {
    let admission_index = root_admission_index(state, invocation_ref)?;
    if state.admissions[admission_index].state != ConversationAdmissionState::Admitted {
        return invalid(format!(
            "invocation {invocation_ref} is not awaiting terminal publication"
        ));
    }
    stage_root_projection_at(
        state,
        admission_index,
        invocation_ref,
        response,
        text,
        reply_kind,
        activity,
    )
}

fn stage_root_projection_at(
    state: &mut ConversationRuntimeState,
    admission_index: usize,
    invocation_ref: &str,
    response: &ActionResponse,
    text: String,
    reply_kind: RootReplyKind,
    activity: Option<(ConversationActivityEvent, u64)>,
) -> CwrResult<Option<ConversationMessage>> {
    let record = state.admissions[admission_index].clone();
    let message = match reply_kind {
        RootReplyKind::None => None,
        reply_kind => {
            let source =
                source_message(state, &record.conversation_ref, &record.source_message_ref)?;
            let target_actor_ref = record
                .target_actor_ref
                .clone()
                .ok_or_else(|| invalid_error("admitted invocation has no target actor"))?;
            let action = match reply_kind {
                RootReplyKind::None => unreachable!("no-message projection was handled above"),
                RootReplyKind::Message => None,
                RootReplyKind::CompletedAction(result) => Some(ConversationActionEvent::Result {
                    activation: source_activation(&source)?,
                    result,
                    outcome: Some(ConversationActionOutcome::Succeeded),
                }),
                RootReplyKind::FailedAction => Some(ConversationActionEvent::Result {
                    activation: source_activation(&source)?,
                    result: serde_json::to_value(response)?,
                    outcome: Some(ConversationActionOutcome::Failed),
                }),
            };
            let visualizations = if response.status.is_completed() {
                visualization::collect_for_invocation(state, invocation_ref, &text)
            } else {
                Vec::new()
            };
            Some(ConversationMessage {
                file_refs: Vec::new(),
                progress: None,
                message_ref: crate::v2026_09::conversation::streaming::finalize_answer(
                    state,
                    invocation_ref,
                    response.status.is_completed(),
                )
                .unwrap_or_else(|| new_ref(&state.runtime_root_ref, "messages")),
                author_actor_ref: if source.addressed_actor_refs.is_empty()
                    && matches!(source.action, Some(ConversationActionEvent::Call { .. }))
                {
                    source.author_actor_ref.clone()
                } else {
                    target_actor_ref
                },
                addressed_actor_refs: vec![source.author_actor_ref],
                addressing_mode: Some(ConversationAddressingMode::Implicit),
                text,
                visualizations,
                reply_to_message_ref: Some(record.source_message_ref.clone()),
                action,
            })
        }
    };
    if let Some(message) = &message {
        state
            .conversations
            .find_mut(|conversation| conversation.conversation_ref == record.conversation_ref)
            .ok_or_else(|| invalid_error("admitted invocation conversation is unavailable"))?
            .record_message(message.clone());
    }
    state.admissions[admission_index].state = admission_state(response)?;
    state.admissions[admission_index].reply_message_ref =
        message.as_ref().map(|message| message.message_ref.clone());
    if let Some((event, occurred_at_unix_ms)) = activity {
        super::invocation::push_execution_activity_in_state(
            state,
            invocation_ref,
            event,
            occurred_at_unix_ms,
        )?;
    }
    Ok(message)
}

fn stage_child_projection(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    response: &ActionResponse,
    text: String,
    occurred_at_unix_ms: u64,
) -> CwrResult<ConversationMessage> {
    stage_child_projection_inner(
        state,
        invocation_ref,
        response,
        text,
        Some(occurred_at_unix_ms),
    )
}

fn stage_child_projection_without_activity(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    response: &ActionResponse,
    text: String,
) -> CwrResult<ConversationMessage> {
    stage_child_projection_inner(state, invocation_ref, response, text, None)
}

fn stage_child_projection_inner(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    response: &ActionResponse,
    text: String,
    occurred_at_unix_ms: Option<u64>,
) -> CwrResult<ConversationMessage> {
    if text.trim().is_empty() {
        return invalid("conversation action result text must not be empty");
    }
    require_child(state, invocation_ref)?;
    let invocation = state
        .invocation_ledger
        .invocation(invocation_ref)
        .cloned()
        .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
    let conversation_ref = invocation
        .conversation_ref
        .clone()
        .ok_or_else(|| invalid_error("child invocation has no conversation ref"))?;
    let source_message_ref = invocation_source_message_ref(state, invocation_ref)?;
    let source = source_message(state, &conversation_ref, &source_message_ref)?;
    let result = match &response.result {
        Some(result) => result.clone(),
        None => serde_json::to_value(response)?,
    };
    let outcome = if response.status.is_completed() {
        ConversationActionOutcome::Succeeded
    } else {
        ConversationActionOutcome::Failed
    };
    let text = if invocation.action_name == RESPOND_TO_MESSAGE_ACTION_NAME
        && response.status.is_completed()
    {
        response
            .result
            .as_ref()
            .and_then(|r| r.get("text"))
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| invalid_error("completed message has no reply text"))?
            .to_owned()
    } else {
        text
    };
    let visualizations = if response.status.is_completed() {
        visualization::collect_for_invocation(state, invocation_ref, &text)
    } else {
        Vec::new()
    };
    let message = ConversationMessage {
        file_refs: Vec::new(),
        progress: None,
        message_ref: crate::v2026_09::conversation::streaming::finalize_answer(
            state,
            invocation_ref,
            response.status.is_completed(),
        )
        .unwrap_or_else(|| new_ref(&state.runtime_root_ref, "messages")),
        // Gateway call messages have no conversational recipient. Attribute
        // their result to the caller whose action exchange it completes.
        author_actor_ref: if source.addressed_actor_refs.is_empty()
            && matches!(source.action, Some(ConversationActionEvent::Call { .. }))
        {
            invocation.requester_actor_ref.clone()
        } else {
            invocation.target_actor_ref
        },
        addressed_actor_refs: vec![invocation.requester_actor_ref],
        addressing_mode: Some(ConversationAddressingMode::Implicit),
        text,
        visualizations,
        reply_to_message_ref: Some(source_message_ref),
        action: if invocation.action_name == RESPOND_TO_MESSAGE_ACTION_NAME {
            None
        } else {
            Some(ConversationActionEvent::Result {
                activation: source_activation(&source)?,
                result,
                outcome: Some(outcome),
            })
        },
    };
    state
        .conversations
        .find_mut(|conversation| conversation.conversation_ref == conversation_ref)
        .ok_or_else(|| invalid_error("child invocation conversation is unavailable"))?
        .record_message(message.clone());
    state
        .invocation_ledger
        .retain_reply_message_ref(invocation_ref, &message.message_ref)?;
    if let Some(occurred_at_unix_ms) = occurred_at_unix_ms {
        let activity = terminal_activity(response)?;
        super::invocation::push_execution_activity_in_state(
            state,
            invocation_ref,
            activity,
            occurred_at_unix_ms,
        )?;
    }
    Ok(message)
}

fn root_admission_index(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<usize> {
    state
        .admissions
        .iter()
        .position(|record| record.invocation_ref.as_deref() == Some(invocation_ref))
        .ok_or_else(|| invalid_error(format!("unknown root invocation ref {invocation_ref}")))
}

fn source_message(
    state: &ConversationRuntimeState,
    conversation_ref: &str,
    source_message_ref: &str,
) -> CwrResult<ConversationMessage> {
    state
        .conversations
        .iter()
        .find(|conversation| conversation.conversation_ref == conversation_ref)
        .and_then(|conversation| {
            conversation
                .messages
                .iter()
                .find(|message| message.message_ref == source_message_ref)
        })
        .cloned()
        .ok_or_else(|| invalid_error("invocation source message is unavailable"))
}

fn source_activation(message: &ConversationMessage) -> CwrResult<String> {
    match &message.action {
        Some(ConversationActionEvent::Call { activation, .. }) => Ok(activation.clone()),
        _ => invalid("direct action invocation has no retained action call"),
    }
}

fn invocation_source_message_ref(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<String> {
    super::invocation::execution_conversation_in_state(state, invocation_ref)?
        .activity
        .iter()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::TurnStarted {
                invocation_ref: started_ref,
                source_message_ref,
                ..
            } if started_ref == invocation_ref => Some(source_message_ref.clone()),
            _ => None,
        })
        .ok_or_else(|| invalid_error("invocation execution conversation has no turn start"))
}

fn retained_terminal_response(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<ActionResponse> {
    state
        .invocation_ledger
        .invocation(invocation_ref)
        .and_then(|invocation| invocation.terminal_response())
        .cloned()
        .ok_or_else(|| invalid_error("terminal invocation has no terminal response"))
}
