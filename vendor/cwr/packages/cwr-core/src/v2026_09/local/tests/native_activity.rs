#[test]
fn synchronous_child_is_the_live_worker_with_root_stop_and_retained_phases() {
    use crate::adapter::{ExecutionActivity, ExecutionPhase, ExecutionPhaseState};
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("state.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "@helper coordinate").unwrap();
    let parent_ref = admission.invocation_ref().unwrap().to_owned();
    let caller = {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claim) =
            state.admission.claim_invocation(&parent_ref, 10).unwrap()
        else {
            panic!("claim parent")
        };
        claim.claim.invocation
    };
    let call = AdapterCall {
        invocation_ref: parent_ref.clone(),
        call_id: "sync".into(),
        operation: INVOKE_ACTION_HOST_OPERATION.into(),
        input: json!({}),
    };
    let AdapterHostCallOutcome::Returned(result) = runtime
        .start_action(
            &call,
            &caller,
            &Rc::new(RefCell::new(VecDeque::new())),
            StartActionInput {
                activation: "@cwr--list--actions".into(),
                input: json!({}),
            },
            None,
        )
        .unwrap();
    let child: StartActionResult = serde_json::from_value(result).unwrap();
    {
        let mut state = runtime.lock().unwrap();
        state
            .admission
            .claim_invocation(&child.invocation_ref, 20)
            .unwrap();
        state
            .admission
            .append_execution_activity(
                &child.invocation_ref,
                ConversationActivityEvent::ExecutionActivity {
                    activity: ExecutionActivity {
                        phase: ExecutionPhase::RestoringContext,
                        state: ExecutionPhaseState::Started,
                        completed_parts: Some(0),
                        total_parts: Some(31),
                    },
                },
            )
            .unwrap();
    }
    let statuses = runtime.conversation_turn_statuses().unwrap();
    let turn = &statuses[admission.conversation_ref()][0];
    assert_eq!(turn.invocation_ref, parent_ref);
    assert_eq!(turn.workers.len(), 1);
    let worker = &turn.workers[0];
    assert_eq!(worker.invocation_ref, child.invocation_ref);
    assert_eq!(
        worker.delegation_path_actor_refs,
        [crate::v2026_09::test_fixture::actor_ref("helper")]
    );
    assert_eq!(worker.started_at_unix_ms, Some(20));
    assert_eq!(worker.phase, ConversationWorkerPhase::RestoringContext);
    assert_eq!(worker.context_parts, Some((0, 31)));
    assert_eq!(
        worker.latest_activity_at_unix_ms,
        worker.phase_started_at_unix_ms
    );
    assert_eq!(runtime.conversation_turn_statuses().unwrap(), statuses);
    let projected = runtime
        .host_conversation(admission.conversation_ref())
        .unwrap()
        .unwrap();
    assert!(projected.activity.iter().any(|item| matches!(&item.event,
        ConversationActivityEvent::WorkLifecycle { invocation_ref, awaited_by_invocation_ref: Some(parent), started_at_unix_ms: Some(20), .. }
            if invocation_ref == &child.invocation_ref && parent == &parent_ref)));
    runtime
        .stop_invocation(&parent_ref, Some("stop nested work".into()))
        .unwrap();
    assert!(runtime.conversation_turn_statuses().unwrap().is_empty());
    let stopped = runtime
        .host_conversation(admission.conversation_ref())
        .unwrap()
        .unwrap();
    for id in [&parent_ref, &child.invocation_ref] {
        assert!(stopped.activity.iter().any(|item| matches!(&item.event,
            ConversationActivityEvent::WorkLifecycle { invocation_ref, state: ActionInvocationState::Stopping,
                stop_requested_at_unix_ms: Some(_), finished_at_unix_ms: None, .. } if invocation_ref == id)));
    }
}

#[test]
fn host_result_is_a_retained_resumption_boundary() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("state.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let (parent, _) = detached_parent_and_child(&runtime);
    let call = AdapterCall {
        invocation_ref: parent.clone(),
        call_id: "resume".into(),
        operation: INVOKE_ACTION_HOST_OPERATION.into(),
        input: json!({}),
    };
    let conversation = runtime
        .conversation_ref_for_invocation(&parent)
        .unwrap()
        .unwrap();
    runtime
        .lock()
        .unwrap()
        .admission
        .retain_host_call_result(&call, Some(json!({})), None, 50_000)
        .unwrap();
    let statuses = runtime.conversation_turn_statuses().unwrap();
    let worker = statuses[&conversation][0]
        .workers
        .iter()
        .find(|w| w.invocation_ref == parent)
        .unwrap();
    assert_eq!(worker.phase, ConversationWorkerPhase::Resuming);
    assert_eq!(worker.latest_activity_at_unix_ms, Some(50_000));
    assert_eq!(worker.turn_started_at_unix_ms, Some(50_000));
}

#[test]
fn concurrent_roots_keep_their_own_latest_activity() {
    use crate::adapter::{ExecutionActivity, ExecutionPhase};
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), temporary.path().join("state.sqlite"), fixture_host_identity(),
    ).unwrap();
    let first = runtime.admit_text(None, "@helper first").unwrap();
    let second = runtime.admit_text(Some(first.conversation_ref()), "@cwr help").unwrap();
    let other = runtime.admit_text(None, "@helper another conversation").unwrap();
    let expected = [
        (&first, ExecutionPhase::Thinking),
        (&second, ExecutionPhase::CompactingContext),
        (&other, ExecutionPhase::RunningCommand),
    ];
    {
        let mut state = runtime.lock().unwrap();
        for (admission, phase) in &expected {
            let invocation = admission.invocation_ref().unwrap();
            state.admission.claim_invocation(invocation, 10).unwrap();
            state
                .admission
                .append_execution_activity(
                    invocation,
                    ConversationActivityEvent::ExecutionActivity {
                        activity: ExecutionActivity::started(*phase),
                    },
                )
                .unwrap();
        }
    }
    let statuses = runtime.conversation_turn_statuses().unwrap();
    assert_eq!(statuses[first.conversation_ref()].len(), 2);
    assert_eq!(statuses[other.conversation_ref()].len(), 1);
    for (admission, phase) in expected {
        let turn = statuses[admission.conversation_ref()].iter()
            .find(|turn| Some(turn.invocation_ref.as_str()) == admission.invocation_ref()).unwrap();
        assert_eq!(turn.workers.len(), 1);
        assert_eq!(turn.workers[0].invocation_ref, turn.invocation_ref);
        assert_eq!(turn.workers[0].phase, phase);
    }
    assert_eq!(runtime.conversation_turn_statuses().unwrap(), statuses);
}

#[test]
fn adapter_tool_events_from_replaced_attempts_are_ignored() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), temporary.path().join("state.sqlite"), fixture_host_identity(),
    ).unwrap();
    let admission = runtime.admit_text(None, "@helper work").unwrap();
    let invocation = admission.invocation_ref().unwrap().to_owned();
    {
        let mut state = runtime.lock().unwrap();
        state.admission.claim_invocation(&invocation, 100).unwrap();
        state.admission.recover_interrupted_invocation(&invocation).unwrap();
        state.admission.claim_invocation(&invocation, 200).unwrap();
    }
    let result = runtime
        .record_adapter_activity(
            &invocation,
            1,
            AdapterActivityEvent::ProviderEvent {
                provider: "codex".into(),
                method: "item/completed".into(),
                data: json!({"item":{"id":"stale","type":"commandExecution","command":"old"}}),
            },
        )
        .unwrap();
    assert!(result.is_none());
}
