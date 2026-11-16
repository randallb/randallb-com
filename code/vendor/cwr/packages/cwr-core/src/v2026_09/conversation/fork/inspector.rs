//! Inspector facts are reconstructed from the already cut-off public snapshot,
//! never from the source's mutable operation records or provider sessions.
use super::*;
use crate::v2026_09::conversation::tools::{ToolAttempt, ToolOperation};

pub(super) fn retain_items<'a>(
    state: &mut ConversationRuntimeState,
    items: impl Iterator<Item = &'a ConversationActivity>,
) {
    for item in items {
        let ConversationActivityEvent::WorkUpdate {
            invocation_ref,
            activity: Some(activity),
            ..
        } = &item.event
        else {
            continue;
        };
        if state
            .invocation_ledger
            .invocation(invocation_ref)
            .is_none_or(|record| record.forked_from_invocation_ref.is_none())
        {
            continue;
        }
        retain_activity(
            state,
            invocation_ref,
            activity,
            item.sequence,
            item.sequence,
        );
    }
}

pub(super) fn retain_activity(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    activity: &WorkActivity,
    first_sequence: u64,
    sequence: u64,
) {
    let attempt_ref = crate::cwpdb::derived_ref("fork_tool_attempt", &[invocation_ref]);
    state.tool_attempts.insert(
        attempt_ref.clone(),
        ToolAttempt {
            reference: attempt_ref.clone(),
            invocation_ref: invocation_ref.to_owned(),
            attempt: 0,
            capture_version: 1,
        },
    );
    // The public history activity already owns its identity as an activity
    // node; inspector operations need an independent canonical identity.
    let reference = crate::cwpdb::derived_ref("fork_tool_operation", &[&activity.activity_ref]);
    let mut activity = activity.clone();
    activity.activity_ref = reference.clone();
    let first_sequence = state
        .tool_operations
        .get(&reference)
        .map_or(first_sequence, |operation| {
            operation.first_sequence.min(first_sequence)
        });
    if let Some(operation) = state.tool_operations.get_mut(&reference) {
        if operation.sequence > sequence {
            operation.first_sequence = first_sequence;
            return;
        }
    }
    state.tool_operations.insert(
        reference.clone(),
        ToolOperation {
            invocation_ref: invocation_ref.to_owned(),
            attempt_ref,
            provider: "retained_history".into(),
            provider_item_id: reference,
            host_call_id: None,
            first_sequence,
            sequence,
            activity,
        },
    );
}

/// Inspector links may reach only work copied into the same destination.
pub(super) fn remap_activity(
    activity: &mut WorkActivity,
    copied: &std::collections::BTreeSet<String>,
    remap: &impl Fn(&str) -> String,
) {
    activity.activity_ref = remap(&activity.activity_ref);
    activity.child_invocation_ref = activity
        .child_invocation_ref
        .as_deref()
        .filter(|reference| copied.contains(*reference))
        .map(remap);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fork_inspector_keeps_execution_order_when_timestamps_move_backwards() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ConversationAdmissionStore::open(
            directory.path().join("clock.sqlite"),
            "cwp://com.example.runtime",
        )
        .unwrap();
        let mut activity = WorkActivity {
            activity_ref: "cwp://com.example.runtime/tool".into(),
            kind: "command".into(),
            label: "Command".into(),
            state: "completed".into(),
            started_at_unix_ms: 200,
            child_invocation_ref: None,
            duration_ms: Some(0),
            command: Some("echo done".into()),
            output: Some("done".into()),
            error: None,
            exit_code: Some(0),
            files: vec![],
        };
        // Timestamp ordering delivered the completed sequence first.
        retain_activity(
            &mut store.state,
            "cwp://com.example.runtime/work",
            &activity,
            5,
            5,
        );
        activity.state = "running".into();
        activity.output = None;
        retain_activity(
            &mut store.state,
            "cwp://com.example.runtime/work",
            &activity,
            3,
            3,
        );
        let retained = store.state.tool_operations.values().next().unwrap();
        assert_eq!(retained.first_sequence, 3);
        assert_eq!(retained.sequence, 5);
        assert_eq!(retained.activity.state, "completed");
        assert_eq!(retained.activity.output.as_deref(), Some("done"));
    }
    #[test]
    fn fork_inspector_child_links_reach_only_copied_work() {
        let mut activity = WorkActivity {
            activity_ref: "cwp://com.example.runtime/tool".into(),
            kind: "command".into(),
            label: "Command".into(),
            state: "completed".into(),
            started_at_unix_ms: 200,
            child_invocation_ref: None,
            duration_ms: Some(0),
            command: Some("echo done".into()),
            output: Some("done".into()),
            error: None,
            exit_code: Some(0),
            files: vec![],
        };
        activity.child_invocation_ref = Some("copied-child".into());
        let copied = std::collections::BTreeSet::from(["copied-child".into()]);
        remap_activity(&mut activity, &copied, &|value| {
            format!("destination/{value}")
        });
        assert_eq!(
            activity.child_invocation_ref.as_deref(),
            Some("destination/copied-child")
        );
        activity.child_invocation_ref = Some("undisclosed-child".into());
        remap_activity(&mut activity, &copied, &|value| {
            format!("destination/{value}")
        });
        assert!(activity.child_invocation_ref.is_none());
    }
}
