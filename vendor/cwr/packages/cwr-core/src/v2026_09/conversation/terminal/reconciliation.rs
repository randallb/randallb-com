pub(super) fn reconcile_terminal_root_projections(
    state: &mut ConversationRuntimeState,
) -> CwrResult<bool> {
    let terminal_roots = state
        .admissions
        .iter()
        .filter_map(|admission| admission.invocation_ref.as_deref())
        .filter_map(|invocation_ref| {
            let invocation = state.invocation_ledger.invocation(invocation_ref)?;
            if !matches!(invocation.origin, InvocationOrigin::Ingress { .. })
                || !invocation.state.is_terminal()
                || invocation.state == ActionInvocationState::Stopping
            {
                return None;
            }
            Some((
                invocation.invocation_ref.clone(),
                invocation.action_name.clone(),
                invocation.terminal_response().cloned(),
                invocation.finished_at_unix_ms,
            ))
        })
        .collect::<Vec<_>>();
    let mut reconciled = false;
    for (invocation_ref, action_name, response, finished_at_unix_ms) in terminal_roots {
        let response = response
            .ok_or_else(|| invalid_error("terminal invocation has no terminal response"))?;
        let finished_at_unix_ms = finished_at_unix_ms
            .ok_or_else(|| invalid_error("terminal invocation has no finish time"))?;
        let admission_index = root_admission_index(state, &invocation_ref)?;
        let has_reply = terminal_reply_in_state(state, admission_index).is_ok();
        let has_terminal_activity = execution_has_terminal_activity(state, &invocation_ref)?;
        let canonical_state = admission_state(&response)?;
        let (text, reply_kind) = reconciliation_reply(&action_name, &response);
        let expects_reply = !matches!(reply_kind, RootReplyKind::None);
        let activity = if has_terminal_activity {
            None
        } else {
            Some((terminal_activity(&response)?, finished_at_unix_ms))
        };

        if !expects_reply {
            // New non-success message ingress is canonically message-less. An
            // older host may already have delivered its retained synthetic
            // failure reply, though, so reconciliation leaves that historical
            // message and reference intact rather than diverging from the
            // external conversation.
            let has_reply_ref = state.admissions[admission_index]
                .reply_message_ref
                .is_some();
            let has_dangling_reply_ref = has_reply_ref && !has_reply;
            if has_terminal_activity
                && state.admissions[admission_index].state == canonical_state
                && !has_dangling_reply_ref
            {
                continue;
            }
            if has_dangling_reply_ref {
                state.admissions[admission_index].reply_message_ref = None;
            }
            state.admissions[admission_index].state = canonical_state;
            if let Some((event, occurred_at_unix_ms)) = activity {
                super::invocation::push_execution_activity_in_state(
                    state,
                    &invocation_ref,
                    event,
                    occurred_at_unix_ms,
                )?;
            }
        } else if has_reply {
            if has_terminal_activity && state.admissions[admission_index].state == canonical_state {
                continue;
            }
            state.admissions[admission_index].state = canonical_state;
            if let Some((event, occurred_at_unix_ms)) = activity {
                super::invocation::push_execution_activity_in_state(
                    state,
                    &invocation_ref,
                    event,
                    occurred_at_unix_ms,
                )?;
            }
        } else {
            stage_root_projection_at(
                state,
                admission_index,
                &invocation_ref,
                &response,
                text,
                reply_kind,
                activity,
            )?;
        }
        reconciled = true;
    }
    Ok(reconciled)
}

fn execution_has_terminal_activity(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<bool> {
    Ok(
        super::invocation::execution_conversation_in_state(state, invocation_ref)?
            .activity
            .iter()
            .any(|activity| {
                matches!(
                    activity.event,
                    ConversationActivityEvent::TurnCompleted { .. }
                        | ConversationActivityEvent::TurnFailed { .. }
                        | ConversationActivityEvent::TurnStopped { .. }
                )
            }),
    )
}

fn reconciliation_reply(action_name: &str, response: &ActionResponse) -> (String, RootReplyKind) {
    if !response.status.is_completed() {
        let reply_kind = if action_name == RESPOND_TO_MESSAGE_ACTION_NAME {
            RootReplyKind::None
        } else {
            RootReplyKind::FailedAction
        };
        return (response.summary.clone(), reply_kind);
    }
    let result = response
        .result
        .clone()
        .expect("validated completed responses retain a result");
    if action_name == RESPOND_TO_MESSAGE_ACTION_NAME {
        let text = serde_json::from_value::<RespondToMessageResult>(result)
            .ok()
            .map(|result| result.text)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| response.summary.clone());
        (text, RootReplyKind::Message)
    } else {
        (
            response.summary.clone(),
            RootReplyKind::CompletedAction(result),
        )
    }
}

