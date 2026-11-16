use super::*;

#[test]
fn shared_reads_provider_phase_cannot_reset_claim_timer_or_progress() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
        .recover_interrupted_invocation(ROOT_INVOCATION_REF)
        .unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 100).unwrap();
    store
        .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 102, "This attempt")
        .unwrap();
    let mut phase =
        crate::adapter::ExecutionActivity::started(crate::adapter::ExecutionPhase::Resuming);
    phase.state = crate::adapter::ExecutionPhaseState::Completed;
    store
        .append_execution_activity(
            ROOT_INVOCATION_REF,
            ConversationActivityEvent::ExecutionActivity { activity: phase },
        )
        .unwrap();
    let snapshot = current(&path);
    let execution = snapshot.active_invocations[0]
        .current_execution
        .as_ref()
        .unwrap();
    assert_eq!(execution.started_at_unix_ms, Some(100));
    assert_eq!(execution.generation, 2);
    assert_eq!(execution.progress.as_deref(), Some("This attempt"));
    // An older resumed record lacks an authoritative boundary. Report unknown
    // instead of calling the first invocation start the current turn start.
    cwpdb_store::test_support::invocation_reads::forget_attempt_boundary(
        &path,
        ROOT_INVOCATION_REF,
    )
    .unwrap();
    let old = current(&path);
    let execution = old.active_invocations[0]
        .current_execution
        .as_ref()
        .unwrap();
    assert_eq!(execution.started_at_unix_ms, None);
    assert_eq!(execution.progress, None);
}

#[test]
fn shared_reads_child_final_reply_keeps_its_own_author_and_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let child_ref = "cwp://com.example.runtime/invocations/final-child";
    start_claimed_child(
        &mut store,
        child_ref,
        "final-child",
        "cwp://com.example.fixture/integrations/codex",
        25,
    );
    let published = store
        .publish_child_action_terminal(
            child_ref,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Child done".into(),
                result: Some(json!({"text":"Child done"})),
                error: None,
            },
            "Child done".into(),
            50,
        )
        .unwrap();
    let PublishInvocationTerminalOutcome::Accepted {
        message: Some(message),
        ..
    } = published
    else {
        panic!()
    };
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
    let child = operation
        .conversation_invocation(CONVERSATION_REF, child_ref)
        .unwrap()
        .unwrap();
    assert_eq!(child.latest_reply.as_ref(), Some(&message));
    assert_eq!(message.author_actor_ref, child.target_actor_ref);
    assert!(child.current_execution.is_none());
    assert!(operation
        .conversation_invocation(CONVERSATION_REF, ROOT_INVOCATION_REF)
        .unwrap()
        .unwrap()
        .latest_reply
        .is_none());
}

#[test]
fn shared_reads_conversational_child_final_is_a_plain_message() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let child_ref = "cwp://com.example.runtime/invocations/reply-child";
    let call = store
        .append_action_call(
            CONVERSATION_REF,
            "cwp://com.example.fixture/actors/worker",
            "cwp://com.example.fixture/actors/child",
            "@child",
            json!({}),
            "Reply please".into(),
        )
        .unwrap();
    let mut request = child_request(child_ref, "reply-child");
    request.action_name = RESPOND_TO_MESSAGE_ACTION_NAME.into();
    let mut adapter = child_adapter_invocation(child_ref);
    adapter.action_name = RESPOND_TO_MESSAGE_ACTION_NAME.into();
    store
        .start_child_invocation(request, &call.message_ref, adapter)
        .unwrap();
    store.claim_invocation(child_ref, 25).unwrap();
    let published = store
        .publish_child_action_terminal(
            child_ref,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Reply".into(),
                result: Some(json!({"text":"Reply"})),
                error: None,
            },
            "Reply".into(),
            50,
        )
        .unwrap();
    let PublishInvocationTerminalOutcome::Accepted {
        message: Some(message),
        ..
    } = published
    else {
        panic!()
    };
    assert!(message.action.is_none());
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
    let child = operation
        .conversation_invocation(CONVERSATION_REF, child_ref)
        .unwrap()
        .unwrap();
    assert_eq!(child.latest_reply.as_ref(), Some(&message));
}

#[test]
fn shared_reads_message_less_terminal_outcomes_preserve_public_fields_only() {
    for status in [503, 409, 499] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("state.sqlite");
        let mut store = open(&path);
        store.commit(prepared_root()).unwrap();
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
        store
            .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 102, "Still working")
            .unwrap();
        assert!(current(&path).active_invocations[0].outcome.is_none());
        let status = ActionResponseStatus::new(status).unwrap();
        if status.is_stopped() {
            store
                .publish_root_stop(
                    ROOT_INVOCATION_REF,
                    InvocationStopRequest {
                        requested_at_unix_ms: 50,
                        requested_by_actor_ref: Some(
                            "cwp://com.example.fixture/actors/person".into(),
                        ),
                        request_id: None,
                        reason: Some("User canceled".into()),
                    },
                    "Stopped".into(),
                )
                .unwrap();
            store
                .settle_stopped_invocation(ROOT_INVOCATION_REF, 50)
                .unwrap();
        } else {
            store
                .publish_root_failure(
                    ROOT_INVOCATION_REF,
                    InvocationTerminal {
                        status,
                        summary: "Cannot finish".into(),
                        result: None,
                        error: Some(
                            ActionResponseDetail::new("unavailable")
                                .with_message("Try again later")
                                .with_details(json!({"private_diagnostic":"never expose"})),
                        ),
                    },
                    "Cannot finish".into(),
                    50,
                )
                .unwrap();
        }
        let retained = store
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .terminal_response()
            .unwrap()
            .clone();
        drop(store);
        let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
        let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
        assert!(operation
            .conversation_current(CONVERSATION_REF)
            .unwrap()
            .active_invocations
            .is_empty());
        let invocation = operation
            .conversation_invocation(CONVERSATION_REF, ROOT_INVOCATION_REF)
            .unwrap()
            .unwrap();
        assert!(invocation.latest_reply.is_none());
        assert!(invocation.current_execution.is_none());
        assert_eq!(invocation.state, status.invocation_state().unwrap());
        assert_eq!(invocation.finished_at_unix_ms, Some(50));
        let outcome = invocation.outcome.unwrap();
        assert_eq!(outcome.status, status);
        assert_eq!(outcome.summary, retained.summary);
        for (actual, expected) in [
            (outcome.error.as_ref(), retained.error.as_ref()),
            (outcome.cause.as_ref(), retained.cause.as_ref()),
        ] {
            assert_eq!(
                actual.map(|detail| &detail.code),
                expected.map(|detail| &detail.code)
            );
            assert_eq!(
                actual.map(|detail| &detail.message),
                expected.map(|detail| &detail.message)
            );
        }
        let serialized = serde_json::to_value(&outcome).unwrap();
        for field in ["error", "cause"] {
            assert!(serialized[field].get("details").is_none());
        }
        assert!(serialized.get("result").is_none());
        assert!(!serialized.to_string().contains("never expose"));
        let decoded: ConversationInvocationOutcome = serde_json::from_value(serialized).unwrap();
        assert_eq!(decoded, outcome);
    }
}
