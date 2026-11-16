/// Converts the removed persisted-await lifecycle into explicit terminal results.
/// Runs inside the caller's atomic store migration, never re-executing a child.
pub(super) fn migrate_suspended_invocations(state: &mut ConversationRuntimeState) -> CwrResult<bool> {
    let occurred_at_unix_ms = current_unix_ms();
    let responses = state.invocation_ledger.settle_legacy_suspended(occurred_at_unix_ms)?;
    let changed = !responses.is_empty();
    for response in responses {
        project_interrupted_response(state, &response, occurred_at_unix_ms)?;
    }
    Ok(changed)
}

pub(super) fn settle_interrupted_execution(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<bool> {
    let execution = super::invocation::execution_conversation_in_state(state, invocation_ref)?;
    let checkpoint = execution.activity.iter().rev().find_map(|activity| match &activity.event {
        ConversationActivityEvent::ProviderEvent { provider, method, data }
            if provider == "codex" && method == "cwr.adapter_checkpoint" => Some(data),
        _ => None,
    });
    let has_host_effects = execution.activity.iter().any(|activity| matches!(
        activity.event, ConversationActivityEvent::HostCall { .. }
            | ConversationActivityEvent::Progress { .. }
            | ConversationActivityEvent::CommentaryProgress { .. }
    ));
    let provider_started = execution.activity.iter().any(|activity| match &activity.event {
        ConversationActivityEvent::ProviderThread { .. } => true,
        ConversationActivityEvent::ProviderEvent { method, .. } => method == "cwr.execution_started",
        _ => false,
    });
    let preparation_recoverable = checkpoint.is_some_and(|data|
        data.get("version").and_then(Value::as_u64) == Some(1)
            && data.get("invocation_ref").and_then(Value::as_str) == Some(invocation_ref)
            && data.get("live_started").and_then(Value::as_bool) == Some(false));
    if !has_host_effects && (!provider_started || preparation_recoverable) {
        return Ok(false);
    }
    let summary = "Execution interrupted. Its live working state cannot be safely restored; completed child results are retained.";
    let finished_at_unix_ms = current_unix_ms();
    let outcome = state.invocation_ledger.finish(invocation_ref, InvocationTerminal {
        status: ActionResponseStatus::FAILED,
        summary: summary.into(), result: None,
        error: Some(ActionResponseDetail::new("execution_interrupted").with_message(summary)),
    }, finished_at_unix_ms)?;
    if let FinishInvocationOutcome::Accepted { response, .. } = outcome {
        project_interrupted_response(state, &response, finished_at_unix_ms)?;
        return Ok(true);
    }
    Ok(false)
}

fn project_interrupted_response(
    state: &mut ConversationRuntimeState,
    response: &ActionResponse,
    occurred_at_unix_ms: u64,
) -> CwrResult<()> {
        let invocation_ref = response.invocation_ref.clone();
        let origin = state.invocation_ledger.invocation(&invocation_ref)
            .ok_or_else(|| invalid_error("migrated invocation is unavailable"))?.origin.clone();
        match origin {
            InvocationOrigin::Ingress { .. } => {
                let reply_kind = root_non_success_reply_kind(state, &invocation_ref)?;
                let activity = terminal_activity(response)?;
                stage_root_projection(state, &invocation_ref, response,
                    response.summary.clone(), reply_kind, Some((activity, occurred_at_unix_ms)))?;
            }
            InvocationOrigin::HostCall { .. } => {
                stage_child_projection(state, &invocation_ref, response,
                    response.summary.clone(), occurred_at_unix_ms)?;
            }
        }
    Ok(())
}
