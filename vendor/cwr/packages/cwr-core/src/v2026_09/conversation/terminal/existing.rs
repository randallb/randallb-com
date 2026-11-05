fn published_message(outcome: PublishInvocationTerminalOutcome) -> CwrResult<ConversationMessage> {
    match outcome {
        PublishInvocationTerminalOutcome::Accepted {
            message: Some(message),
            ..
        }
        | PublishInvocationTerminalOutcome::AlreadyTerminal {
            message: Some(message),
            ..
        } => Ok(message),
        PublishInvocationTerminalOutcome::Accepted { message: None, .. }
        | PublishInvocationTerminalOutcome::AlreadyTerminal { message: None, .. } => {
            invalid("terminal invocation has no published conversation result")
        }
    }
}

fn published_optional_message(
    outcome: PublishInvocationTerminalOutcome,
) -> CwrResult<Option<ConversationMessage>> {
    match outcome {
        PublishInvocationTerminalOutcome::Accepted { message, .. }
        | PublishInvocationTerminalOutcome::AlreadyTerminal { message, .. } => Ok(message),
    }
}

fn required_message(message: Option<ConversationMessage>) -> CwrResult<ConversationMessage> {
    message.ok_or_else(|| invalid_error("terminal invocation has no published conversation result"))
}

fn claim_if_admitted(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    started_at_unix_ms: u64,
) -> CwrResult<()> {
    state.invocation_ledger.claim_person_reply(invocation_ref, started_at_unix_ms)?;
    let invocation = state
        .invocation_ledger
        .invocation(invocation_ref)
        .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
    if invocation.state == ActionInvocationState::Admitted
        && !matches!(
            state
                .invocation_ledger
                .claim(invocation_ref, started_at_unix_ms)?,
            ClaimInvocationOutcome::Claimed(_)
        )
    {
        return invalid(format!(
            "invocation {invocation_ref} could not be claimed for terminal publication"
        ));
    }
    Ok(())
}

fn existing_root_terminal(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<Option<PublishInvocationTerminalOutcome>> {
    let admission_index = root_admission_index(state, invocation_ref)?;
    match state.admissions[admission_index].state {
        ConversationAdmissionState::Admitted => Ok(None),
        ConversationAdmissionState::Recorded => invalid(format!(
            "invocation {invocation_ref} was not admitted for execution"
        )),
        ConversationAdmissionState::Completed
        | ConversationAdmissionState::Failed
        | ConversationAdmissionState::Stopped => {
            Ok(Some(PublishInvocationTerminalOutcome::AlreadyTerminal {
                response: retained_terminal_response(state, invocation_ref)?,
                message: terminal_reply_in_state(state, admission_index).ok(),
            }))
        }
    }
}

fn existing_root_stop(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
    request: &InvocationStopRequest,
) -> CwrResult<Option<PublishInvocationStopOutcome>> {
    let admission_index = root_admission_index(state, invocation_ref)?;
    if state.invocation_ledger.retained_stop_outcome(invocation_ref, request)?.is_none() {
        return Ok(None);
    }
    match state.admissions[admission_index].state {
        ConversationAdmissionState::Admitted => Ok(None),
        ConversationAdmissionState::Recorded => invalid(format!(
            "invocation {invocation_ref} was not admitted for execution"
        )),
        ConversationAdmissionState::Completed
        | ConversationAdmissionState::Failed
        | ConversationAdmissionState::Stopped => match state
            .invocation_ledger
            .retained_stop_outcome(invocation_ref, request)?
            .ok_or_else(|| invalid_error("terminal root has no retained stop outcome"))?
        {
            StopInvocationOutcome::Accepted {
                response,
                stopped_invocation_refs,
                woken_invocation_refs,
                ..
            } => {
                for stopped_ref in &stopped_invocation_refs {
                    if stopped_ref != invocation_ref
                        && existing_child_result(state, stopped_ref)?.is_none()
                    {
                        return Ok(None);
                    }
                }
                Ok(Some(PublishInvocationStopOutcome::Accepted {
                    response,
                    message: existing_root_reply(state, invocation_ref)?,
                    stopped_invocation_refs,
                    woken_invocation_refs,
                }))
            }
            StopInvocationOutcome::TooLate { response } => {
                Ok(Some(PublishInvocationStopOutcome::TooLate {
                    response,
                    message: terminal_reply_in_state(state, admission_index).ok(),
                }))
            }
            StopInvocationOutcome::Unknown => {
                invalid("terminal root invocation disappeared during stop replay")
            }
        },
    }
}

fn existing_child_terminal(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<Option<PublishInvocationTerminalOutcome>> {
    let invocation = state
        .invocation_ledger
        .invocation(invocation_ref)
        .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
    if !invocation.state.is_terminal() {
        return Ok(None);
    }
    Ok(Some(PublishInvocationTerminalOutcome::AlreadyTerminal {
        response: retained_terminal_response(state, invocation_ref)?,
        message: existing_child_result(state, invocation_ref)?,
    }))
}

fn existing_child_stop(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
    request: &InvocationStopRequest,
) -> CwrResult<Option<PublishInvocationStopOutcome>> {
    let Some(invocation) = state.invocation_ledger.invocation(invocation_ref) else {
        return Ok(Some(PublishInvocationStopOutcome::Unknown));
    };
    if !invocation.state.is_terminal() {
        return Ok(None);
    }
    if state.invocation_ledger.retained_stop_outcome(invocation_ref, request)?.is_none() {
        return Ok(None);
    }
    match state
        .invocation_ledger
        .retained_stop_outcome(invocation_ref, request)?
        .ok_or_else(|| invalid_error("terminal child has no retained stop outcome"))?
    {
        StopInvocationOutcome::Accepted {
            response,
            stopped_invocation_refs,
            woken_invocation_refs,
            ..
        } => Ok(
            existing_child_result(state, invocation_ref)?.map(|message| {
                PublishInvocationStopOutcome::Accepted {
                    response,
                    message: Some(message),
                    stopped_invocation_refs,
                    woken_invocation_refs,
                }
            }),
        ),
        StopInvocationOutcome::TooLate { response } => {
            Ok(Some(PublishInvocationStopOutcome::TooLate {
                response,
                message: existing_child_result(state, invocation_ref)?,
            }))
        }
        StopInvocationOutcome::Unknown => {
            invalid("terminal child invocation disappeared during stop replay")
        }
    }
}

fn require_child(state: &ConversationRuntimeState, invocation_ref: &str) -> CwrResult<()> {
    let invocation = state
        .invocation_ledger
        .invocation(invocation_ref)
        .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
    if !matches!(&invocation.origin, InvocationOrigin::HostCall { .. }) {
        return invalid(format!("invocation {invocation_ref} is not a child action"));
    }
    Ok(())
}

fn root_non_success_reply_kind(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<RootReplyKind> {
    let invocation = state
        .invocation_ledger
        .invocation(invocation_ref)
        .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
    if invocation.action_name == RESPOND_TO_MESSAGE_ACTION_NAME {
        Ok(RootReplyKind::None)
    } else {
        Ok(RootReplyKind::FailedAction)
    }
}

fn legacy_root_non_success_reply_kind(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<RootReplyKind> {
    let admission_index = root_admission_index(state, invocation_ref)?;
    let record = &state.admissions[admission_index];
    let source = source_message(state, &record.conversation_ref, &record.source_message_ref)?;
    if matches!(source.action, Some(ConversationActionEvent::Call { .. })) {
        Ok(RootReplyKind::FailedAction)
    } else {
        Ok(RootReplyKind::None)
    }
}

