fn require_active_caller(
    admission: &ConversationAdmissionStore,
    call: &AdapterCall,
) -> CwrResult<()> {
    let caller = admission
        .invocation_record(&call.invocation_ref)
        .ok_or_else(|| invalid_error(format!("unknown caller {}", call.invocation_ref)))?;
    if caller.state != ActionInvocationState::Running || !caller.attempt_active {
        return invalid(format!(
            "caller {} has no active attempt",
            call.invocation_ref
        ));
    }
    Ok(())
}

fn ensure_host_call(
    admission: &mut ConversationAdmissionStore,
    call: &AdapterCall,
) -> CwrResult<u64> {
    if let Some(occurred_at_unix_ms) = retained_host_call_time(admission, call)? {
        return Ok(occurred_at_unix_ms);
    }
    let activity = admission.append_execution_activity(
        &call.invocation_ref,
        ConversationActivityEvent::HostCall {
            call_id: call.call_id.clone(),
            operation: call.operation.clone(),
            input: call.input.clone(),
        },
    )?;
    Ok(activity.occurred_at_unix_ms)
}

fn retained_host_call_time(
    admission: &ConversationAdmissionStore,
    call: &AdapterCall,
) -> CwrResult<Option<u64>> {
    let execution_ref = admission
        .execution_conversation_ref(&call.invocation_ref)
        .ok_or_else(|| invalid_error("caller invocation has no execution conversation"))?;
    let execution = admission
        .conversation_history(execution_ref)?
        .ok_or_else(|| invalid_error("caller invocation has no execution conversation"))?;
    let mut retained = None;
    for activity in &execution.activity {
        let ConversationActivityEvent::HostCall {
            call_id,
            operation,
            input,
        } = &activity.event
        else {
            continue;
        };
        if call_id != &call.call_id {
            continue;
        }
        if operation != &call.operation || input != &call.input {
            return invalid(format!(
                "host call id {} was reused with different input",
                call.call_id
            ));
        }
        if retained.replace(activity.occurred_at_unix_ms).is_some() {
            return invalid(format!(
                "host call {} was retained more than once",
                call.call_id
            ));
        }
    }
    Ok(retained)
}

fn retained_stop_matches(
    target: &ActionInvocationRecord,
    caller_actor_ref: &str,
    reason: Option<&str>,
    request_id: &str,
    retained_call_time: u64,
) -> bool {
    matches!(
        target.state,
        ActionInvocationState::Stopping | ActionInvocationState::Stopped
    ) && target.stop_requested_by_actor_ref.as_deref() == Some(caller_actor_ref)
        && target.stop_request_id.as_deref() == Some(request_id)
        && target.stop_reason.as_deref() == reason
        && target.stop_requested_at_unix_ms == Some(retained_call_time)
}

fn retained_stopped_subtree(
    admission: &ConversationAdmissionStore,
    target: &ActionInvocationRecord,
    caller_actor_ref: &str,
    reason: Option<&str>,
    request_id: &str,
) -> Vec<String> {
    admission
        .invocation_records()
        .filter(|candidate| {
            matches!(
                candidate.state,
                ActionInvocationState::Stopping | ActionInvocationState::Stopped
            ) && candidate.stop_requested_at_unix_ms == target.stop_requested_at_unix_ms
                && candidate.stop_requested_by_actor_ref.as_deref() == Some(caller_actor_ref)
                && candidate.stop_request_id.as_deref() == Some(request_id)
                && candidate.stop_reason.as_deref() == reason
                && is_attached_descendant_or_self(admission, candidate, &target.invocation_ref)
        })
        .map(|candidate| candidate.invocation_ref.clone())
        .collect()
}

fn is_attached_descendant_or_self(
    admission: &ConversationAdmissionStore,
    candidate: &ActionInvocationRecord,
    target_ref: &str,
) -> bool {
    if candidate.invocation_ref == target_ref {
        return true;
    }
    let mut current = candidate;
    loop {
        let Some(parent_ref) = current.parent_invocation_ref.as_deref() else {
            return false;
        };
        if parent_ref == target_ref {
            return true;
        }
        let Some(parent) = admission.invocation_record(parent_ref) else {
            return false;
        };
        current = parent;
    }
}

fn retained_woken_waiters(
    _admission: &ConversationAdmissionStore,
    _stopped_invocation_refs: &[String],
) -> Vec<String> { Vec::new() }

fn require_action_depth(
    admission: &ConversationAdmissionStore,
    caller: &ActionInvocationRecord,
) -> CwrResult<()> {
    let mut depth = 0;
    let mut current = caller;
    while let Some(parent_ref) = current.parent_invocation_ref.as_deref() {
        depth += 1;
        if depth >= MAX_ACTION_DEPTH {
            return invalid(format!(
                "action_depth_exceeded: nesting is limited to {MAX_ACTION_DEPTH}"
            ));
        }
        current = admission
            .invocation_record(parent_ref)
            .ok_or_else(|| invalid_error("invocation parent is unavailable"))?;
    }
    Ok(())
}

fn latest_execution_activity(
    admission: &ConversationAdmissionStore,
    invocation_ref: &str,
) -> CwrResult<ConversationActivity> {
    admission
        .execution_conversation(invocation_ref)
        .and_then(|conversation| conversation.activity.last())
        .cloned()
        .ok_or_else(|| invalid_error("execution conversation has no activity"))
}

fn parse_input<T: serde::de::DeserializeOwned>(label: &str, input: &Value) -> CwrResult<T> {
    serde_json::from_value(input.clone())
        .map_err(|error| invalid_error(format!("invalid {label} input: {error}")))
}

fn action_call_text(activation: &str, input: &Value) -> CwrResult<String> {
    if input.as_object().is_some_and(Map::is_empty) {
        Ok(activation.into())
    } else {
        Ok(format!("{activation} {}", serde_json::to_string(input)?))
    }
}

fn actor_handle(actor_ref: &str) -> CwrResult<String> {
    let name = actor_ref
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty() && !name.contains('#'))
        .ok_or_else(|| invalid_error(format!("actor ref {actor_ref} has no local handle")))?;
    if !name.chars().enumerate().all(|(index, character)| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit() && index > 0
            || character == '_' && index > 0
    }) {
        return invalid(format!(
            "actor ref {actor_ref} must end in a lowercase snake-case handle"
        ));
    }
    Ok(format!("@{name}"))
}
