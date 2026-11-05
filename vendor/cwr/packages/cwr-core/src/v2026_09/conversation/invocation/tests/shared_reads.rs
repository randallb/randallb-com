use super::*;
use std::time::Duration;

fn current(path: &Path) -> ConversationCurrentSnapshot {
    ConversationReadStore::open(path, RUNTIME_ROOT)
        .unwrap()
        .begin_operation(Duration::from_secs(5))
        .unwrap()
        .conversation_current(CONVERSATION_REF)
        .unwrap()
}

fn preview(generation: u64) -> ConversationExecutionOutput {
    ConversationExecutionOutput {
        invocation_ref: ROOT_INVOCATION_REF.into(),
        generation,
        target_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
        phase: crate::adapter::ExecutionPhase::Responding,
        text: "Temporary output".into(),
    }
}

#[test]
fn shared_reads_resolve_canonical_message_and_invocation_resources() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();

    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
    let message = operation
        .conversation_resource(SOURCE_MESSAGE_REF)
        .unwrap()
        .unwrap();
    assert_eq!(message.position.conversation_ref, CONVERSATION_REF);
    assert!(matches!(
        message.resource,
        ConversationResource::Message { message, .. }
            if message.message_ref == SOURCE_MESSAGE_REF
    ));

    let invocation = operation
        .conversation_resource(ROOT_INVOCATION_REF)
        .unwrap()
        .unwrap();
    assert_eq!(invocation.position.conversation_ref, CONVERSATION_REF);
    assert!(matches!(
        invocation.resource,
        ConversationResource::Invocation { invocation }
            if invocation.invocation_ref == ROOT_INVOCATION_REF
                && invocation.current_execution
                    .as_ref()
                    .is_some_and(|execution| execution.generation == 1)
    ));
    assert!(operation
        .conversation_resource("cwp://com.example.runtime/messages/missing")
        .unwrap()
        .is_none());
}

#[test]
fn shared_reads_history_keeps_commentary_once_and_live_progress_out() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    let ConversationInvocationClaimOutcome::Claimed(claim) =
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap()
    else {
        panic!()
    };
    let item = commentary(&claim.model_run_ref, "turn", "item", "Same text");
    store
        .record_commentary_progress(ROOT_INVOCATION_REF, 1, item.clone())
        .unwrap();
    store
        .record_commentary_progress(ROOT_INVOCATION_REF, 1, item)
        .unwrap();
    store
        .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 102, "Temporary progress")
        .unwrap();
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
    let first = operation
        .conversation_history(
            CONVERSATION_REF,
            ConversationRecordWindow {
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(first.total_count, 2);
    assert!(first.content[0].origin_invocation_ref.is_none());
    assert!(first.has_next_page);
    let next = operation
        .conversation_history(
            CONVERSATION_REF,
            ConversationRecordWindow {
                after: Some(&first.content[0].key),
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(next.content.len(), 1);
    assert_eq!(
        next.content[0].origin_invocation_ref.as_deref(),
        Some(ROOT_INVOCATION_REF)
    );
    assert_eq!(next.content[0].message.text, "Same text");
    assert_eq!(
        next.content[0]
            .message
            .progress
            .as_ref()
            .unwrap()
            .invocation_ref,
        ROOT_INVOCATION_REF
    );
    let older = operation
        .conversation_history(
            CONVERSATION_REF,
            ConversationRecordWindow {
                before: Some(&next.content[0].key),
                last: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(older.content, first.content);
    // Existing consumers still receive their old commentary-filtered content.
    assert_eq!(
        operation
            .conversation_content(
                CONVERSATION_REF,
                ConversationRecordWindow {
                    first: Some(10),
                    ..Default::default()
                }
            )
            .unwrap()
            .total_count,
        1
    );
    let snapshot = operation.conversation_current(CONVERSATION_REF).unwrap();
    let root = &snapshot.active_invocations[0];
    assert!(root.outcome.is_none());
    assert_eq!(root.latest_reply.as_ref(), Some(&next.content[0].message));
    assert_eq!(
        root.current_execution.as_ref().unwrap().progress.as_deref(),
        Some("Temporary progress")
    );
    assert_eq!(root.source_message_ref.as_deref(), Some(SOURCE_MESSAGE_REF));
}

#[path = "shared_reads/browse.rs"]
mod browse;

#[test]
fn shared_reads_snapshot_is_consistent_during_completion_and_empty_after() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
    let before = operation.conversation_current(CONVERSATION_REF).unwrap();
    store
        .publish_root_message_completed(
            ROOT_INVOCATION_REF,
            ActionResponseStatus::COMPLETED,
            json!({"text":"Done"}),
            "Done".into(),
            50,
        )
        .unwrap();
    assert_eq!(
        operation.conversation_current(CONVERSATION_REF).unwrap(),
        before
    );
    drop(operation);
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
    let mut after = operation.conversation_current(CONVERSATION_REF).unwrap();
    assert!(after.active_invocations.is_empty());
    assert_ne!(after.position, before.position);
    assert!(!after.apply_live_output(preview(1)));
    let finished = operation
        .conversation_invocation(CONVERSATION_REF, ROOT_INVOCATION_REF)
        .unwrap()
        .unwrap();
    assert_eq!(finished.state, ActionInvocationState::Completed);
    assert_eq!(finished.finished_at_unix_ms, Some(50));
    assert!(finished.current_execution.is_none());
    let outcome = finished.outcome.as_ref().unwrap();
    assert_eq!(outcome.status, ActionResponseStatus::COMPLETED);
    assert_eq!(outcome.summary, "Done");
    assert!(outcome.error.is_none());
    assert!(outcome.cause.is_none());
    assert!(serde_json::to_value(outcome)
        .unwrap()
        .get("result")
        .is_none());
    assert_eq!(finished.latest_reply.as_ref().unwrap().text, "Done");
    assert!(operation
        .conversation_invocation(
            CONVERSATION_REF,
            "cwp://com.example.runtime/invocations/missing"
        )
        .unwrap()
        .is_none());
}

#[test]
fn shared_reads_resume_resets_execution_not_invocation_and_fences_preview() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    assert!(current(&path).active_invocations[0]
        .current_execution
        .is_none());
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let mut first = current(&path);
    assert!(first.apply_live_output(preview(1)));
    let mut cleared = preview(1);
    cleared.text.clear();
    assert!(first.apply_live_output(cleared));
    assert!(first.active_invocations[0]
        .current_execution
        .as_ref()
        .unwrap()
        .live_output
        .is_none());
    store
        .recover_interrupted_invocation(ROOT_INVOCATION_REF)
        .unwrap();
    assert!(!current(&path).apply_live_output(preview(1)));
    store.claim_invocation(ROOT_INVOCATION_REF, 100).unwrap();
    let mut resumed = current(&path);
    let root = &resumed.active_invocations[0];
    assert_eq!(root.started_at_unix_ms, Some(11));
    let execution = root.current_execution.as_ref().unwrap();
    assert_eq!(execution.generation, 2);
    assert_eq!(execution.started_at_unix_ms, Some(100));
    assert_eq!(execution.latest_activity_at_unix_ms, Some(100));
    assert!(!resumed.apply_live_output(preview(1)));
    let mut wrong_actor = preview(2);
    wrong_actor.target_actor_ref = "other".into();
    assert!(!resumed.apply_live_output(wrong_actor));
    assert!(resumed.apply_live_output(preview(2)));
}

#[test]
fn shared_reads_parallel_children_are_distinct_without_implied_waits() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    for suffix in ["one", "two"] {
        start_claimed_child(
            &mut store,
            &format!("{RUNTIME_ROOT}/invocations/{suffix}"),
            suffix,
            "cwp://com.example.fixture/integrations/codex",
            25,
        );
    }
    let snapshot = current(&path);
    assert_eq!(snapshot.active_invocations.len(), 3);
    let root = snapshot
        .active_invocations
        .iter()
        .find(|r| r.invocation_ref == ROOT_INVOCATION_REF)
        .unwrap();
    assert_eq!(root.child_invocation_refs.len(), 2);
    assert!(root.waits.is_empty());
    assert_eq!(
        snapshot.active_invocations[1].target_actor_ref,
        snapshot.active_invocations[2].target_actor_ref
    );
    assert_ne!(
        snapshot.active_invocations[1].invocation_ref,
        snapshot.active_invocations[2].invocation_ref
    );
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
    let newest = operation
        .conversation_history(
            CONVERSATION_REF,
            ConversationRecordWindow {
                last: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(newest
        .content
        .iter()
        .all(|row| row.message.message_ref != SOURCE_MESSAGE_REF));
    assert_eq!(
        operation.conversation_current(CONVERSATION_REF).unwrap(),
        snapshot
    );
}

#[test]
fn shared_reads_waiting_parent_and_detached_children_remain_discoverable() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let child_ref = "cwp://com.example.runtime/invocations/waited-child";
    start_claimed_child(
        &mut store,
        child_ref,
        "waited-call",
        "cwp://com.example.fixture/integrations/codex",
        25,
    );
    // Legacy explicit waits remain readable even though current synchronous
    // host calls no longer create this retained wait form.
    cwpdb_store::test_support::invocation_reads::private_wait(
        &path,
        ROOT_INVOCATION_REF,
        child_ref,
        "cwp://com.example.runtime/waits/one",
    )
    .unwrap();
    let snapshot = current(&path);
    let parent = snapshot
        .active_invocations
        .iter()
        .find(|r| r.invocation_ref == ROOT_INVOCATION_REF)
        .unwrap();
    assert_eq!(parent.state, ActionInvocationState::Waiting);
    assert_eq!(parent.waits.len(), 1);
    assert_eq!(parent.waits[0].awaited_invocation_ref, child_ref);
    assert_eq!(
        parent.current_execution.as_ref().unwrap().phase,
        crate::adapter::ExecutionPhase::Waiting
    );
    assert!(!serde_json::to_string(&snapshot)
        .unwrap()
        .contains("never expose"));
    // A child remains in the active set even when its parent is terminal.
    cwpdb_store::test_support::invocation_reads::finish_parent(&path, ROOT_INVOCATION_REF).unwrap();
    let detached = current(&path);
    assert_eq!(detached.active_invocations.len(), 1);
    assert_eq!(detached.active_invocations[0].invocation_ref, child_ref);
    assert_eq!(
        detached.active_invocations[0]
            .parent_invocation_ref
            .as_deref(),
        Some(ROOT_INVOCATION_REF)
    );
}

#[test]
fn shared_reads_scopes_invocations_and_honors_cancellation() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    let other = "cwp://com.example.runtime/conversations/other";
    cwpdb_store::test_support::invocation_reads::empty_conversation(&path, RUNTIME_ROOT, other)
        .unwrap();
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
    assert!(operation
        .conversation_current(other)
        .unwrap()
        .active_invocations
        .is_empty());
    assert!(operation
        .conversation_invocation(other, ROOT_INVOCATION_REF)
        .unwrap()
        .is_none());
    operation.cancellation().cancel();
    assert!(operation.conversation_current(CONVERSATION_REF).is_err());
    assert!(operation
        .conversation_history(
            CONVERSATION_REF,
            ConversationRecordWindow {
                first: Some(1),
                ..Default::default()
            }
        )
        .is_err());
}

#[path = "shared_reads_regressions.rs"]
mod regressions;

#[path = "shared_reply_identity.rs"]
mod shared_reply_identity;

#[path = "shared_reads/browse_limits.rs"]
mod browse_limits;

#[test]
fn shared_reads_retain_stopping_across_reopen_until_explicit_settlement() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("stopping.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
        .stop_invocation(ROOT_INVOCATION_REF, InvocationStopRequest::new(12))
        .unwrap();
    drop(store);
    let mut store = open(&path);
    assert!(store.recover_interrupted_invocations().unwrap().is_empty());
    for _ in 0..2 {
        let snapshot = current(&path);
        let invocation = &snapshot.active_invocations[0];
        assert_eq!(invocation.state, ActionInvocationState::Stopping);
        assert_eq!(invocation.finished_at_unix_ms, None);
        assert!(invocation.stop_settlement_tracked);
        assert!(invocation.current_execution.is_some());
    }
    store
        .settle_stopped_invocation(ROOT_INVOCATION_REF, 13)
        .unwrap();
    let snapshot = current(&path);
    assert!(snapshot.active_invocations.is_empty());
}

#[test]
fn canonical_resource_port_matches_indexed_public_projection_through_lifecycle() {
    for rollout in [
        ConversationTimelineRollout::Disabled,
        ConversationTimelineRollout::NewConversations,
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("resource-port.sqlite");
        let mut store =
            ConversationAdmissionStore::open_with_timeline_rollout(&path, RUNTIME_ROOT, rollout)
                .unwrap();
        store.commit(prepared_root()).unwrap();
        let compare = |store: &ConversationAdmissionStore, reference: &str| {
            let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
            let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
            let expected = operation.conversation_resource(reference).unwrap();
            let actual = store.canonical_conversation_resource(reference).unwrap();
            assert_eq!(actual, expected, "resource {reference} in {rollout:?}");
            actual
        };
        compare(&store, SOURCE_MESSAGE_REF);
        compare(&store, ROOT_INVOCATION_REF);
        compare(&store, "cwp://com.example.runtime/missing");
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
        store
            .record_invocation_progress_with_activity(
                ROOT_INVOCATION_REF,
                102,
                "Current attempt progress",
            )
            .unwrap();
        compare(&store, ROOT_INVOCATION_REF);
        let child = "cwp://com.example.runtime/invocations/resource-port-child";
        start_claimed_child(
            &mut store,
            child,
            "resource-port-child",
            "cwp://com.example.fixture/integrations/codex",
            25,
        );
        compare(&store, child);
        let outcome = store
            .publish_child_action_terminal(
                child,
                InvocationTerminal {
                    status: ActionResponseStatus::COMPLETED,
                    summary: "Child completed".into(),
                    result: Some(json!({"text":"Child reply"})),
                    error: None,
                },
                "Child reply".into(),
                50,
            )
            .unwrap();
        let PublishInvocationTerminalOutcome::Accepted {
            message: Some(message),
            ..
        } = outcome
        else {
            panic!("child reply required")
        };
        compare(&store, &message.message_ref);
        compare(&store, child);
        store
            .publish_root_message_completed(
                ROOT_INVOCATION_REF,
                ActionResponseStatus::COMPLETED,
                json!({"text":"Root reply"}),
                "Root completed".into(),
                70,
            )
            .unwrap();
        compare(&store, ROOT_INVOCATION_REF);
        compare(&store, child);
    }
}

#[path = "shared_reads/pages.rs"]
mod pages;

#[path = "shared_reads/timeline_port.rs"]
mod timeline_port;

#[path = "shared_reads/history_cold.rs"]
mod history_cold;

#[path = "shared_reads/public_bridge.rs"]
mod public_bridge;
