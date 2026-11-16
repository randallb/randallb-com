fn stage_commentary_progress(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    attempt_count: u64,
    commentary: AdapterCompletedCommentary,
    occurred_at_unix_ms: u64,
    provider_integration_ref: String,
    retained: Option<ConversationActivity>,
) -> CwrResult<RecordCommentaryProgressOutcome> {
    let invocation = state
        .invocation_ledger
        .invocation(invocation_ref)
        .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
    let is_current_model_run = invocation.state == ActionInvocationState::Running
        && invocation.attempt_active
        && invocation.attempt_count == attempt_count
        && state.streaming_runs.get(invocation_ref).is_none_or(|run|
            run.model_run_ref == commentary.model_run_ref && !run.closed);
    if let Some(activity) = retained {
        let ConversationActivityEvent::CommentaryProgress {
            provider,
            thread_id,
            text,
            ..
        } = &activity.event
        else {
            unreachable!("commentary filter only returns commentary progress")
        };
        if provider != &commentary.provider
            || thread_id != &commentary.thread_id
            || text != &commentary.text
        {
            if !is_current_model_run {
                return Ok(RecordCommentaryProgressOutcome::IgnoredStaleModelRun);
            }
            return invalid("provider commentary item was replayed with different content");
        }
        return Ok(RecordCommentaryProgressOutcome::Existing(activity.clone()));
    }

    if !is_current_model_run {
        return Ok(RecordCommentaryProgressOutcome::IgnoredStaleModelRun);
    }

    let public_text = commentary.text.clone();
    let activity = push_execution_activity_in_state(
        state,
        invocation_ref,
        ConversationActivityEvent::CommentaryProgress {
            model_run_ref: commentary.model_run_ref,
            provider_integration_ref,
            provider: commentary.provider,
            thread_id: commentary.thread_id,
            response_or_turn_id: commentary.response_or_turn_id,
            item_id: commentary.item_id,
            text: commentary.text,
        },
        occurred_at_unix_ms,
    )?;
    super::working::publish_progress(state, invocation_ref, &activity, public_text, None)?;
    Ok(RecordCommentaryProgressOutcome::Recorded(activity))
}

fn validate_completed_commentary(commentary: &AdapterCompletedCommentary) -> CwrResult<()> {
    CwrRef::parse(&commentary.model_run_ref)
        .map_err(|error| invalid_error(format!("commentary model run ref is invalid: {error}")))?;
    for (label, value) in [
        ("provider", commentary.provider.as_str()),
        (
            "response or turn ID",
            commentary.response_or_turn_id.as_str(),
        ),
        ("item ID", commentary.item_id.as_str()),
    ] {
        if value.trim().is_empty() {
            return invalid(format!("commentary {label} must not be empty"));
        }
    }
    if commentary.text.trim().is_empty() {
        return invalid("commentary text must not be empty");
    }
    if commentary
        .thread_id
        .as_deref()
        .is_some_and(|thread_id| thread_id.trim().is_empty())
    {
        return invalid("commentary thread ID must not be empty");
    }
    Ok(())
}

fn commentary_provider_integration(
    state: &ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<String> {
    let execution = execution_conversation_in_state(state, invocation_ref)?;
    execution
        .activity
        .iter()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::TurnStarted {
                invocation_ref: started_ref,
                invocation: Some(snapshot),
                ..
            } if started_ref == invocation_ref => Some(snapshot.clone()),
            _ => None,
        })
        .ok_or_else(|| invalid_error("commentary progress has no retained adapter invocation"))
        .and_then(|snapshot| {
            serde_json::from_value::<AdapterInvocation>(snapshot).map_err(Into::into)
        })?
        .integration_ref
        .ok_or_else(|| invalid_error("commentary progress has no provider integration ref"))
}
