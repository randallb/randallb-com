//! Public message metadata is resolved from exact retained associations.
use super::*;
use crate::v2026_09::conversation::{
    ConversationActivity, ConversationActivityEvent, ConversationMessage, WorkActivity,
};

#[derive(Debug, Clone)]
pub struct ConversationMessageMetadata {
    pub message: Box<ConversationMessage>,
    pub occurred_at_unix_ms: Option<i64>,
    pub invocation_ref: Option<String>,
    pub streaming_phase: Option<String>,
    pub progress_occurred_at_unix_ms: Option<u64>,
    progress_activity: Option<Box<WorkActivity>>,
    progress_source_sequence: Option<u64>,
    pub progress_summary: Option<String>,
    pub progress_response_status: Option<u16>,
    pub measured_at_unix_ms: u64,
    pub action_status: Option<u16>,
    pub action_summary: Option<String>,
}
#[derive(Debug, Clone)]
pub struct ConversationActorPresentation {
    pub actor_ref: String,
    pub name: Option<String>,
    pub handle: Option<String>,
    pub coworker: bool,
}
impl ConversationPublicationSnapshot {
    pub fn message_metadata(
        &self,
        reference: &str,
    ) -> CwrResult<Option<ConversationMessageMetadata>> {
        self.read(|op| {
            self.cache
                .metadata
                .get_or_load(reference, || self.load_message_metadata(op, reference))
        })
    }
    fn load_message_metadata(
        &self,
        op: &ConversationReadOperation<'_>,
        reference: &str,
    ) -> CwrResult<Option<ConversationMessageMetadata>> {
        let Some(resource) = self.resource_in(op, reference)? else {
            return Ok(None);
        };
        let ConversationResource::Message {
            message,
            occurred_at_unix_ms,
            streaming,
        } = resource.resource
        else {
            return Ok(None);
        };
        let conversation = resource.position.conversation_ref;
        let mut value = ConversationMessageMetadata {
            message,
            occurred_at_unix_ms: occurred_at_unix_ms.filter(|ms| *ms > 0),
            invocation_ref: None,
            streaming_phase: None,
            progress_occurred_at_unix_ms: None,
            progress_activity: None,
            progress_source_sequence: None,
            progress_summary: None,
            progress_response_status: None,
            measured_at_unix_ms: self.measured_at_unix_ms,
            action_status: None,
            action_summary: None,
        };
        if streaming {
            let (phase, invocation) = op
                .finite_snapshot()?
                .streaming_message_metadata(reference)?;
            value.streaming_phase = phase;
            value.invocation_ref = Some(invocation);
        } else if let Some(progress) = value.message.progress.clone() {
            load_progress(op, &progress, &mut value)?;
        } else if value.message.action.is_some() {
            value.invocation_ref = action_invocation(op, &conversation, reference)?;
        }
        let Some(reference) = value.invocation_ref.as_deref() else {
            return Ok(Some(value));
        };
        let Some(resource) = self.resource_in(op, reference)? else {
            value.invocation_ref = None;
            return Ok(Some(value));
        };
        if let ConversationResource::Invocation { invocation } = resource.resource {
            let belongs =
                progress_in_latest_attempt(op, reference, value.progress_source_sequence)?;
            if belongs {
                value.progress_activity = latest_activity(op, reference, value.progress_activity)?;
            }
            let active = invocation
                .current_execution
                .as_ref()
                .is_some_and(|execution| execution.attempt_active);
            if value
                .progress_activity
                .as_ref()
                .is_some_and(|activity| activity.duration_ms.is_none() && (!belongs || !active))
            {
                value.progress_activity = None;
            }
            if let Some(outcome) = invocation.outcome {
                value.action_status = Some(outcome.status.as_u16());
                value.action_summary = Some(outcome.summary);
            }
        } else {
            value.invocation_ref = None;
        }
        Ok(Some(value))
    }
}
fn action_invocation(
    op: &ConversationReadOperation<'_>,
    conversation: &str,
    message: &str,
) -> CwrResult<Option<String>> {
    op.finite_snapshot()?
        .message_action_invocation(conversation, message)
}
fn load_progress(
    op: &ConversationReadOperation<'_>,
    progress: &crate::v2026_09::conversation::ConversationProgress,
    value: &mut ConversationMessageMetadata,
) -> CwrResult<()> {
    value.invocation_ref = Some(progress.invocation_ref.clone());
    let Some(record) = op
        .finite_snapshot()?
        .progress_activity(&progress.invocation_ref, &progress.source_activity_ref)?
    else {
        return Ok(());
    };
    let activity: ConversationActivity = serde_json::from_value(record)?;
    value.progress_source_sequence = Some(activity.sequence);
    value.progress_occurred_at_unix_ms =
        (activity.occurred_at_unix_ms > 0).then_some(activity.occurred_at_unix_ms);
    match activity.event {
        ConversationActivityEvent::WorkUpdate { activity, .. } => {
            value.progress_activity = activity
        }
        ConversationActivityEvent::Progress {
            summary, status, ..
        } if progress.status == Some(status) && status.as_u16() < 200 => {
            value.progress_summary = Some(summary);
            value.progress_response_status = Some(status.as_u16());
        }
        _ => {}
    }
    Ok(())
}
fn progress_in_latest_attempt(
    op: &ConversationReadOperation<'_>,
    invocation: &str,
    sequence: Option<u64>,
) -> CwrResult<bool> {
    op.finite_snapshot()?
        .progress_in_latest_attempt(invocation, sequence)
}
fn latest_activity(
    op: &ConversationReadOperation<'_>,
    invocation: &str,
    current: Option<Box<WorkActivity>>,
) -> CwrResult<Option<Box<WorkActivity>>> {
    let Some(current) = current else {
        return Ok(None);
    };
    op.finite_snapshot()?
        .latest_work_activity(invocation, &current.activity_ref)?
        .map_or(Ok(Some(current)), |record| {
            Ok(Some(serde_json::from_value(record)?))
        })
}

#[derive(Debug, Clone)]
pub struct ConversationActivityTiming {
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: Option<u64>,
    pub measured_at_unix_ms: u64,
    pub elapsed_seconds: f64,
}
impl ConversationMessageMetadata {
    pub fn progress_timing(&self) -> CwrResult<Option<ConversationActivityTiming>> {
        let Some(activity) = self.progress_activity.as_deref() else {
            return Ok(None);
        };
        if activity.started_at_unix_ms == 0 {
            return Ok(None);
        }
        let finished = activity
            .duration_ms
            .map(|duration| {
                activity
                    .started_at_unix_ms
                    .checked_add(duration)
                    .ok_or_else(|| invalid_error("Activity time overflow"))
            })
            .transpose()?;
        if finished.is_none() && activity.state != "running" {
            return Ok(None);
        }
        let measured = finished.unwrap_or(self.measured_at_unix_ms);
        Ok(Some(ConversationActivityTiming {
            started_at_unix_ms: activity.started_at_unix_ms,
            finished_at_unix_ms: finished,
            measured_at_unix_ms: measured,
            elapsed_seconds: measured.saturating_sub(activity.started_at_unix_ms) as f64 / 1000.0,
        }))
    }
}
