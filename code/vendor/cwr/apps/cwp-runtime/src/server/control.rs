use super::*;

pub(super) fn stop_conversation(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    conversation_ref: String,
    requesting_actor_ref: String,
    invocation_ref: Option<String>,
    event_at_unix_ms: Option<u64>,
) -> CwrResult<()> {
    let runtime = lock_state(state)?.runtime.clone();
    let reason = Some("Stopped from a connected host".into());
    let outcome = runtime
        .bundled_conversation_module_as(
            "cwp://!cwr/actor_integrations/host",
            &requesting_actor_ref,
        )?
        .stop(
            &conversation_ref,
            invocation_ref.as_deref(),
            &request_id,
            event_at_unix_ms,
            reason,
        )?;
    let stopped = match outcome {
        LocalStopOutcome::Accepted {
            stopped_invocation_refs,
            ..
        } => stopped_invocation_refs,
        LocalStopOutcome::TooLate { .. } | LocalStopOutcome::Unknown => Vec::new(),
    };
    let acknowledgement = command_succeeded(writer, request_id, None, stopped.clone(), None);
    let publication = publish_stop_updates(state, &runtime, &stopped, Some(&conversation_ref));
    acknowledgement.and(publication)
}

pub(super) fn publish_stop_updates(
    state: &SharedState,
    runtime: &LocalRuntime,
    stopped: &[String],
    conversation_ref: Option<&str>,
) -> CwrResult<()> {
    close_live_invocations(state, stopped)?;
    let mut conversations = std::collections::BTreeSet::new();
    conversations.extend(conversation_ref.map(str::to_owned));
    for invocation_ref in stopped {
        conversations.extend(runtime.conversation_ref_for_invocation(invocation_ref)?);
    }
    for conversation_ref in &conversations {
        let _ = broadcast_conversation(state, runtime, conversation_ref)?;
    }
    status::broadcast_turn_statuses_for(state, runtime, &conversations)?;
    for invocation_ref in stopped {
        if let Some((conversation_ref, target_actor_ref, response)) =
            runtime.terminal_execution_for_invocation(invocation_ref)?
        {
            super::broadcast_execution_terminal(
                state,
                &conversation_ref,
                &target_actor_ref,
                Box::new(response),
            )?;
        }
    }
    Ok(())
}
