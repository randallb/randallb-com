use cwr::v2026_09::conversation::ConversationActivity;

use super::*;

const CONVERSATION_REF: &str = "cwp://example.test/runtime/conversations/one";
const INVOCATION_A: &str = "cwp://example.test/runtime/invocations/a";
const INVOCATION_B: &str = "cwp://example.test/runtime/invocations/b";
fn target_ref() -> String {
    crate::test_refs::actor_ref("helper")
}

#[test]
fn submitted_stop_clears_live_output_without_an_execution_worker() {
    let temporary = tempfile::tempdir().unwrap();
    let state = crate::server::tests::runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let root = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "live-intrinsic-stop",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@helper hold this request",
        )
        .unwrap();
    let invocation_ref = root.invocation_ref().unwrap();
    {
        let mut state = lock_state(&state).unwrap();
        state
            .live_outputs
            .observe_runtime_event(root.conversation_ref(), &started(invocation_ref));
        state.live_outputs.apply_stream_snapshot(
            None,
            Some(&output(invocation_ref, "partial", "Still working")),
            &[],
        );
        assert!(!state.live_outputs.snapshot().is_empty());
    }
    let (_client, server) = UnixStream::pair().unwrap();
    server
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    execute_command(
        &state,
        &client_writer(server).unwrap(),
        RuntimeServiceCommand::ExecuteIntrinsic {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "stop-live-output".into(),
            operation_id: None,
            command: cwr::intrinsic_cwr::IntrinsicCwrCommand::StopInvocation {
                invocation_ref: Some(invocation_ref.into()),
            },
            requesting_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            conversation_ref: Some(root.conversation_ref().into()),
        },
    )
    .unwrap();
    assert!(lock_state(&state)
        .unwrap()
        .live_outputs
        .snapshot()
        .is_empty());
}

#[test]
fn completed_commentary_is_delivered_before_a_same_tick_clear() {
    let mut registry = LiveOutputRegistry::default();
    registry.observe_runtime_event(CONVERSATION_REF, &started(INVOCATION_A));
    let partial = output(INVOCATION_A, "item-a", "Checking");
    registry.apply_stream_snapshot(None, Some(&partial), &[]);

    let completed = commentary(INVOCATION_A, "item-a", "Checking the workspace");
    registry.observe_runtime_event(CONVERSATION_REF, &completed);
    registry.observe_runtime_event(CONVERSATION_REF, &terminal(INVOCATION_A));

    let (required, current) = registry.broadcast_snapshot();
    let changes = broadcast_changes(&BTreeMap::new(), &required, &current);
    assert_eq!(
        changes,
        vec![
            LiveOutputChange {
                conversation_ref: CONVERSATION_REF.into(),
                output: Some(projected(INVOCATION_A, "Checking the workspace")),
            },
            LiveOutputChange {
                conversation_ref: CONVERSATION_REF.into(),
                output: None,
            },
        ]
    );

    registry.acknowledge_required_changes(required[0].delivery_sequence);
    assert!(registry.broadcast_snapshot().0.is_empty());
}

#[test]
fn native_stop_clears_only_the_stopped_invocations_progress() {
    let mut registry = LiveOutputRegistry::default();
    registry.observe_runtime_event(CONVERSATION_REF, &started(INVOCATION_A));
    registry.observe_runtime_event(CONVERSATION_REF, &started(INVOCATION_B));
    registry.apply_stream_snapshot(None, Some(&output(INVOCATION_A, "a", "Checking A")), &[]);
    registry.apply_stream_snapshot(None, Some(&output(INVOCATION_B, "b", "Checking B")), &[]);

    assert_eq!(
        registry.close_invocations([INVOCATION_B]),
        vec![LiveOutputChange {
            conversation_ref: CONVERSATION_REF.into(),
            output: Some(projected(INVOCATION_A, "Checking A")),
        }]
    );
    assert_eq!(
        registry.close_invocations([INVOCATION_A]),
        vec![LiveOutputChange {
            conversation_ref: CONVERSATION_REF.into(),
            output: None,
        }]
    );
    assert!(registry.snapshot().is_empty());
}

#[test]
fn completed_commentary_delivery_is_idempotent() {
    let mut registry = LiveOutputRegistry::default();
    registry.observe_runtime_event(CONVERSATION_REF, &started(INVOCATION_A));
    let completed = commentary(INVOCATION_A, "item-a", "Checking the workspace");

    registry.observe_runtime_event(CONVERSATION_REF, &completed);
    registry.observe_runtime_event(CONVERSATION_REF, &completed);

    assert_eq!(registry.broadcast_snapshot().0.len(), 1);
}

#[test]
fn closed_model_run_clears_output_without_a_terminal_event() {
    let mut registry = LiveOutputRegistry::default();
    registry.observe_runtime_event(CONVERSATION_REF, &started(INVOCATION_A));
    let partial = output(INVOCATION_A, "item-a", "Checking");
    registry.apply_stream_snapshot(None, Some(&partial), &[]);

    let changes = registry.apply_stream_snapshot(
        None,
        None,
        &[AdapterClosedModelRun {
            invocation_ref: INVOCATION_A.into(),
            model_run_ref: partial.model_run_ref,
        }],
    );

    assert_eq!(
        changes,
        vec![LiveOutputChange {
            conversation_ref: CONVERSATION_REF.into(),
            output: None,
        }]
    );
    assert!(registry.snapshot().is_empty());
}

#[test]
fn one_invocation_cannot_clear_another_invocations_output() {
    let mut registry = LiveOutputRegistry::default();
    registry.observe_runtime_event(CONVERSATION_REF, &started(INVOCATION_A));
    registry.observe_runtime_event(CONVERSATION_REF, &started(INVOCATION_B));
    let output_a = output(INVOCATION_A, "item-a", "A");
    let output_b = output(INVOCATION_B, "item-b", "B");
    registry.apply_stream_snapshot(None, Some(&output_a), &[]);
    registry.apply_stream_snapshot(None, Some(&output_b), &[]);

    assert!(registry
        .apply_stream_snapshot(Some(&output_a), None, &[])
        .is_empty());
    assert_eq!(registry.snapshot()[0].1.invocation_ref, INVOCATION_B);
}

#[test]
fn newest_output_falls_back_to_another_active_invocation() {
    let mut registry = LiveOutputRegistry::default();
    registry.observe_runtime_event(CONVERSATION_REF, &started(INVOCATION_A));
    registry.observe_runtime_event(CONVERSATION_REF, &started(INVOCATION_B));
    let output_a = output(INVOCATION_A, "item-a", "A");
    let output_b = output(INVOCATION_B, "item-b", "B");
    registry.apply_stream_snapshot(None, Some(&output_a), &[]);
    registry.apply_stream_snapshot(None, Some(&output_b), &[]);

    let changes = registry.close_invocations([INVOCATION_B]);
    assert_eq!(
        changes[0].output.as_ref().unwrap().invocation_ref,
        INVOCATION_A
    );
    assert_eq!(registry.snapshot()[0].1.text, "A");
}

#[test]
fn broadcaster_coalesces_updates_and_emits_explicit_clears() {
    let first = BTreeMap::from([(CONVERSATION_REF.into(), projected(INVOCATION_A, "A"))]);
    let second = BTreeMap::from([(CONVERSATION_REF.into(), projected(INVOCATION_B, "newest"))]);

    let changed = presentation_changes(&first, &second);
    assert_eq!(changed[0].output.as_ref().unwrap().text, "newest");
    assert_eq!(
        presentation_changes(&second, &BTreeMap::new()),
        vec![LiveOutputChange {
            conversation_ref: CONVERSATION_REF.into(),
            output: None,
        }]
    );
}

#[test]
fn broadcaster_captures_live_output_after_joining_publication_order() {
    let temporary = tempfile::tempdir().unwrap();
    let state = crate::server::tests::runtime_state(&temporary);
    let (client, server) = UnixStream::pair().unwrap();
    let writer = client_writer(server).unwrap();
    subscribe(&state, &writer).unwrap();
    let old = output(INVOCATION_A, "item-a", "old");
    let new = output(INVOCATION_A, "item-a", "new");
    {
        let mut runtime_state = lock_state(&state).unwrap();
        runtime_state
            .live_outputs
            .observe_runtime_event(CONVERSATION_REF, &started(INVOCATION_A));
        runtime_state
            .live_outputs
            .apply_stream_snapshot(None, Some(&old), &[]);
    }

    let publication = lock_broadcast(&state).unwrap();
    let publisher_state = Arc::clone(&state);
    let (started_publishing, publishing_started) = std::sync::mpsc::channel();
    let (completed, completion) = std::sync::mpsc::channel();
    let publisher = std::thread::spawn(move || {
        started_publishing.send(()).unwrap();
        let mut published = BTreeMap::new();
        completed
            .send(publish_live_output_changes(
                &publisher_state,
                &mut published,
            ))
            .unwrap();
    });
    publishing_started.recv().unwrap();
    assert!(completion.recv_timeout(Duration::from_millis(25)).is_err());
    lock_state(&state)
        .unwrap()
        .live_outputs
        .apply_stream_snapshot(Some(&old), Some(&new), &[]);
    drop(publication);
    completion
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    publisher.join().unwrap();

    let event: RuntimeServiceEvent =
        serde_json::from_str(&BufReader::new(client).lines().next().unwrap().unwrap()).unwrap();
    assert!(matches!(
        event,
        RuntimeServiceEvent::ConversationLiveOutput {
            output: Some(output),
            ..
        } if output.text == "new"
    ));
    disconnect_writer(&writer);
}

include!("test_support.rs");
