#[test]
fn conversation_stop_request_signals_the_adapter_and_replays_without_stopping_new_work() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("conversation-admission.json");
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "@helper check this").unwrap();
    let invocation_ref = admission.invocation_ref().unwrap();
    let control = ProcessAdapterStopControl::new();
    {
        let mut state = runtime.lock().unwrap();
        state
            .admission
            .claim_invocation(invocation_ref, current_unix_ms())
            .unwrap();
        state.active.insert(invocation_ref.into(), control.clone());
    }

    assert!(runtime
        .bundled_conversation_module_as("cwp://!cwr/actor_integrations/host", "~@/actors/observer")
        .unwrap()
        .stop(
            admission.conversation_ref(),
            None,
            "unauthorized-stop",
            None,
            None
        )
        .is_err());
    assert!(!control.is_requested());
    let LocalStopOutcome::Accepted {
        stopped_invocation_refs,
        ..
    } = runtime
        .bundled_conversation_module_as("cwp://!cwr/actor_integrations/host", crate::v2026_09::test_fixture::actor_ref("person"))
        .unwrap()
        .stop(
            admission.conversation_ref(),
            None,
            "native-stop",
            None,
            None,
        )
        .unwrap()
    else {
        panic!("stop must be accepted")
    };
    assert_eq!(stopped_invocation_refs, [invocation_ref]);
    assert!(
        control.is_requested(),
        "the process must receive cancellation, not just a UI change"
    );
    assert!(runtime.conversation_turn_statuses().unwrap().is_empty());
    drop(runtime);

    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
    )
    .unwrap();
    let next = runtime
        .admit_text(Some(admission.conversation_ref()), "@helper next turn")
        .unwrap();
    let LocalStopOutcome::Accepted {
        stopped_invocation_refs,
        ..
    } = runtime
        .bundled_conversation_module_as("cwp://!cwr/actor_integrations/host", crate::v2026_09::test_fixture::actor_ref("person"))
        .unwrap()
        .stop(
            admission.conversation_ref(),
            None,
            "native-stop",
            None,
            None,
        )
        .unwrap()
    else {
        panic!("same stop must replay as accepted")
    };
    assert_eq!(stopped_invocation_refs, [invocation_ref]);
    assert_eq!(
        runtime
            .turn(&next.record)
            .unwrap()
            .invocation
            .unwrap()
            .state,
        ActionInvocationState::Admitted
    );
}

#[test]
fn selected_stop_checks_authority_conversation_and_replay_target() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("state.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let first = runtime.admit_text(None, "@helper first").unwrap();
    let queued = runtime
        .admit_text(Some(first.conversation_ref()), "@helper queued")
        .unwrap();
    let other = runtime.admit_text(None, "@helper other").unwrap();
    let stop = |conversation: &str, invocation: &str, actor: &str| {
        runtime.bundled_conversation_module_as("cwp://!cwr/actor_integrations/host", actor).unwrap()
            .stop(conversation, Some(invocation), "selected-stop", None, None)
    };
    let selected = queued.invocation_ref().unwrap();
    assert!(stop(first.conversation_ref(), selected, "~@/actors/observer").is_err());
    assert!(stop(other.conversation_ref(), selected, crate::v2026_09::test_fixture::actor_ref("person")).is_err());
    for _ in 0..2 {
        let LocalStopOutcome::Accepted {
            stopped_invocation_refs,
            ..
        } = stop(first.conversation_ref(), selected, crate::v2026_09::test_fixture::actor_ref("person")).unwrap()
        else {
            panic!("selected stop must be accepted and replayable")
        };
        assert_eq!(stopped_invocation_refs, [selected]);
    }
    assert!(stop(
        first.conversation_ref(),
        first.invocation_ref().unwrap(),
        crate::v2026_09::test_fixture::actor_ref("person")
    )
    .is_err());
    let first_record = runtime.turn(&first.record).unwrap().invocation.unwrap();
    assert_eq!(first_record.state, ActionInvocationState::Admitted);
    assert_eq!(first_record.attempt_count, 0);
    let queued_record = runtime.turn(&queued.record).unwrap().invocation.unwrap();
    assert_eq!(queued_record.state, ActionInvocationState::Stopped);
    assert_eq!(queued_record.attempt_count, 0);
}
