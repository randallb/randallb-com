#[test]
fn readiness_failure_after_stop_settles_executor_and_retains_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), temporary.path().join("runtime.sqlite"), fixture_host_identity(),
    ).unwrap();
    let admission = runtime.admit_text(None, "@helper readiness race").unwrap();
    let reference = admission.invocation_ref().unwrap();
    let record = {
        let mut state = runtime.lock().unwrap();
        state.admission.claim_invocation(reference, current_unix_ms()).unwrap();
        state.active.insert(reference.into(), ProcessAdapterStopControl::default());
        state.admission.invocation_record(reference).unwrap().clone()
    };
    runtime.stop_invocation(reference, None).unwrap();
    assert_eq!(runtime.turn(&admission.record).unwrap().invocation.unwrap().state,
        ActionInvocationState::Stopping);
    runtime.publish_claim_provider_unavailable(
        &record,
        AdapterReadiness {
            status: crate::adapter::AdapterReadinessStatus::SetupRequired,
            code: "fixture_unavailable".into(), message: "Fixture unavailable".into(),
            repair: None,
        },
        &Rc::new(RefCell::new(VecDeque::new())), &mut |_| {},
    ).unwrap();
    let settled = runtime.turn(&admission.record).unwrap().invocation.unwrap();
    assert_eq!(settled.state, ActionInvocationState::Stopped);
    assert!(settled.finished_at_unix_ms.is_some());
    assert!(!runtime.lock().unwrap().active.contains_key(reference));
    assert_eq!(runtime.take_settled_stop_invocations().unwrap(), vec![reference]);
    assert!(runtime.take_settled_stop_invocations().unwrap().is_empty());
}

#[test]
fn child_settlement_retains_both_child_and_parent_for_host_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), temporary.path().join("runtime.sqlite"), fixture_host_identity(),
    ).unwrap();
    let admission = runtime.admit_text(None, "@helper parent").unwrap();
    let parent_ref = admission.invocation_ref().unwrap().to_owned();
    let child_ref = {
        let mut state = runtime.lock().unwrap();
        let parent = match state.admission.claim_invocation(&parent_ref, 10).unwrap() {
            ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
            ConversationInvocationClaimOutcome::NotRunnable { .. } => {
                panic!("new invocation was not runnable")
            }
        };
        let child_ref = new_invocation_ref(state.admission.runtime_root_ref());
        let call_message = state
            .admission
            .append_host_action_call(ConversationHostActionCall {
                conversation_ref: admission.conversation_ref(),
                caller_invocation_ref: &parent_ref,
                host_call_id: "start-child",
                nested_invocation_ref: &child_ref,
                caller_actor_ref: &parent.target_actor_ref,
                target_actor_ref: &parent.target_actor_ref,
                activation: "@helper--draft--brief",
                input: json!({}),
                text: "@helper--draft--brief".into(),
            })
            .unwrap();
        state
            .admission
            .start_child_invocation(
                StartActionInvocation {
                    invocation_ref: child_ref.clone(),
                    origin: InvocationOrigin::HostCall {
                        caller_invocation_ref: parent_ref.clone(),
                        call_id: "start-child".into(),
                    },
                    requester_actor_ref: parent.target_actor_ref.clone(),
                    target_actor_ref: parent.target_actor_ref.clone(),
                    conversation_ref: Some(admission.conversation_ref().into()),
                    actor_integration_ref: parent.actor_integration_ref.clone(),
                    action_name: "draft--brief".into(),
                    input: json!({}),
                    parent_invocation_ref: Some(parent_ref.clone()),
                    admitted_at_unix_ms: 20,
                },
                &call_message.message_ref,
                AdapterInvocation::new(
                    child_ref.clone(),
                    "draft--brief",
                    json!({}),
                    json!({"type": "object"}),
                ),
            )
            .unwrap();

        child_ref
    };
    runtime.lock().unwrap().admission.claim_invocation(&child_ref, 21).unwrap();
    runtime.stop_invocation(&parent_ref, None).unwrap();
    runtime.settle_stopped_execution(&parent_ref).unwrap();
    assert!(runtime.take_settled_stop_invocations().unwrap().is_empty());
    runtime.settle_stopped_execution(&child_ref).unwrap();
    assert_eq!(runtime.take_settled_stop_invocations().unwrap(), vec![child_ref, parent_ref]);
    assert!(runtime.take_settled_stop_invocations().unwrap().is_empty());
}

#[test]
fn replayed_intrinsic_stop_reuses_the_retained_request_timestamp() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), temporary.path().join("runtime.sqlite"), fixture_host_identity(),
    ).unwrap();
    let target = runtime.admit_text(None, "@helper queued").unwrap();
    let control = runtime.admit_action(Some(target.conversation_ref()), "@cwr--stop--invocation",
        json!({"invocation_ref": target.invocation_ref().unwrap()})).unwrap();
    let record = runtime.lock().unwrap().admission.invocation_record(control.invocation_ref().unwrap()).unwrap().clone();
    let first = runtime.stop_previous_intrinsic_invocation(&record, target.invocation_ref()).unwrap();
    let original = runtime.turn(&target.record).unwrap().invocation.unwrap();
    thread::sleep(Duration::from_millis(5));
    let replay = runtime.stop_previous_intrinsic_invocation(&record, target.invocation_ref()).unwrap();
    assert!(matches!(first, IntrinsicCwrResponse::InvocationStopped { .. }));
    assert_eq!(first, replay);
    let retained = runtime.turn(&target.record).unwrap().invocation.unwrap();
    assert_eq!(retained.stop_requested_at_unix_ms, original.stop_requested_at_unix_ms);
    assert_eq!(retained.stop_request_id, original.stop_request_id);
}

#[test]
fn archive_waits_for_retained_stopping_work_after_reopening() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("runtime.sqlite");
    let runtime = LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity()).unwrap();
    let admission = runtime.admit_text(None, "@helper pending").unwrap();
    let reference = admission.invocation_ref().unwrap().to_owned();
    runtime.lock().unwrap().admission.claim_invocation(&reference, current_unix_ms()).unwrap();
    runtime.stop_invocation(&reference, None).unwrap();
    drop(runtime);
    let runtime = LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity()).unwrap();
    let archiver = runtime.clone();
    let conversation = admission.conversation_ref().to_owned();
    let thread = thread::spawn(move || archiver.set_conversation_archived_with_stops_as(
        Some(&conversation), crate::v2026_09::test_fixture::actor_ref("person"), true));
    let deadline = Instant::now() + Duration::from_secs(2);
    while runtime.lock().unwrap().archiving_conversations.is_empty() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!thread.is_finished());
    assert!(!runtime.conversation(admission.conversation_ref()).unwrap().unwrap().archived);
    // Explicit executor-exit confirmation is required even after a restart.
    runtime.settle_stopped_execution(&reference).unwrap();
    thread.join().unwrap().unwrap();
    assert!(runtime.conversation(admission.conversation_ref()).unwrap().unwrap().archived);
}
