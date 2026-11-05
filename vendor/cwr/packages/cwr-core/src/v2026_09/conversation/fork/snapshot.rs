//! Copies immutable identity and reconstructs lifecycle facts from retained events.
use super::*;
use crate::v2026_09::invocation::{ActionInvocationRecord, ActionResponse, ActionResponseDetail};
use std::collections::BTreeSet;

pub(in crate::v2026_09::conversation) fn freeze(
    original: &ActionInvocationRecord,
    execution: Option<&RetainedConversation>,
    messages: &BTreeSet<String>,
    time: u64,
) -> ActionInvocationRecord {
    let mut record = original.clone();
    record.forked_from_invocation_ref = Some(original.invocation_ref.clone());
    record.source_message_ref = record.source_message_ref.filter(|r| messages.contains(r));
    record.actor_integration_ref = None;
    record.input = Value::Null;
    // Previously copied work is already immutable. Re-forking it preserves that
    // published snapshot; it has no live execution conversation to replay.
    if original.forked_from_invocation_ref.is_some() {
        return record;
    }
    record.attempt_active = false;
    record.attempt_started_at_unix_ms = None;
    record.attempt_started_after_activity_sequence = None;
    record.stop_execution_pending = false;
    record.stop_requested_at_unix_ms = None;
    record.stop_requested_by_actor_ref = None;
    record.stop_request_id = None;
    record.stop_reason = None;
    record.stop_settlement_tracked = true;
    record.started_at_unix_ms = None;
    let mut terminal = None;
    // Execution activities retain event order, independently of wall clocks.
    // Provider records and mutable ledger responses are deliberately excluded.
    for item in execution
        .into_iter()
        .flat_map(|execution| &execution.activity)
    {
        let outcome = match &item.event {
            ConversationActivityEvent::TurnCompleted {
                status, summary, ..
            } => Some((*status, summary.clone())),
            ConversationActivityEvent::TurnFailed { status, error } => {
                Some((*status, error.clone()))
            }
            ConversationActivityEvent::TurnStopped { status, reason } => Some((
                *status,
                reason.clone().unwrap_or_else(|| "Work stopped.".into()),
            )),
            _ => None,
        };
        let observed_work = outcome
            .as_ref()
            .is_some_and(|(status, _)| !status.is_stopped())
            || match &item.event {
                ConversationActivityEvent::Progress { .. }
                | ConversationActivityEvent::CommentaryProgress { .. } => true,
                ConversationActivityEvent::ExecutionActivity { activity } => {
                    activity.phase != crate::adapter::ExecutionPhase::Queued
                }
                _ => false,
            };
        if observed_work && record.started_at_unix_ms.is_none() {
            record.started_at_unix_ms = Some(item.occurred_at_unix_ms);
        }
        if let Some((status, summary)) = outcome {
            terminal = Some((status, summary, item.occurred_at_unix_ms));
        }
    }
    let (status, summary, finished) = terminal.unwrap_or_else(|| {
        (
            ActionResponseStatus::STOPPED,
            "Work was unfinished at the fork boundary.".into(),
            time,
        )
    });
    record.attempt_count = u64::from(record.started_at_unix_ms.is_some());
    record.state = status
        .invocation_state()
        .unwrap_or(ActionInvocationState::Stopped);
    record.finished_at_unix_ms = Some(finished);
    if record.state == ActionInvocationState::Stopped {
        record.stop_requested_at_unix_ms = Some(finished);
    }
    record.responses = vec![ActionResponse {
        progress: None,
        invocation_ref: original.invocation_ref.clone(),
        sequence: 1,
        status,
        summary: if summary.trim().is_empty() {
            "Retained work history.".into()
        } else {
            summary
        },
        // Public messages carry disclosed results. The invocation copy must not
        // expose raw provider result/error details through its inspector.
        result: status.is_completed().then_some(Value::Null),
        error: (status.is_failed() || status.is_blocked())
            .then(|| ActionResponseDetail::new("Retained work failure.")),
        cause: None,
    }];
    record
}

/// Project only retained execution events. Mutable wait state must not travel
/// backward across the cutoff; parent relations still come from retained calls.
pub(super) fn public_work(
    store: &ConversationAdmissionStore,
    source: &RetainedConversation,
    executions: &[RetainedConversation],
) -> Vec<ConversationActivity> {
    let by_invocation = executions
        .iter()
        .flat_map(|execution| {
            execution
                .activity
                .iter()
                .filter_map(move |item| match &item.event {
                    ConversationActivityEvent::TurnStarted { invocation_ref, .. } => {
                        Some((invocation_ref.as_str(), execution))
                    }
                    _ => None,
                })
        })
        .collect::<BTreeMap<_, _>>();
    crate::v2026_09::host::project_authoritative_work_activities(
        source,
        executions.iter(),
        store.invocation_records(),
        &[],
        |reference| by_invocation.get(reference).copied(),
        |reference| store.invocation_record(reference).is_some(),
    )
}
