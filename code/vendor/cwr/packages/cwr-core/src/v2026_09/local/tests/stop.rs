#[test]
fn active_stop_follows_parent_links() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let (parent_ref, child_ref) = detached_parent_and_child(&runtime);

    let response = runtime
        .stop_active_intrinsic_invocation(&child_ref, None)
        .unwrap();
    assert_eq!(
        response,
        crate::intrinsic_cwr::IntrinsicCwrResponse::InvocationStopped {
            invocation_ref: parent_ref.clone(),
            stopped_invocation_refs: vec![parent_ref.clone(), child_ref.clone()],
        }
    );
    let state = runtime.lock().unwrap();
    assert_eq!(
        state
            .admission
            .invocation_record(&parent_ref)
            .unwrap()
            .state,
        ActionInvocationState::Stopping
    );
    assert_eq!(
        state.admission.invocation_record(&child_ref).unwrap().state,
        ActionInvocationState::Stopped
    );
}

#[test]
fn active_stop_does_not_target_a_terminal_ingress_above_detached_work() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let (parent_ref, child_ref) = detached_parent_and_child(&runtime);
    runtime
        .lock()
        .unwrap()
        .admission
        .finish_invocation(
            &parent_ref,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "parent finished".into(),
                result: Some(json!({})),
                error: None,
            },
            30,
        )
        .unwrap();

    let response = runtime
        .stop_active_intrinsic_invocation(&child_ref, None)
        .unwrap();
    assert_eq!(
        response,
        crate::intrinsic_cwr::IntrinsicCwrResponse::InvocationStopped {
            invocation_ref: child_ref.clone(),
            stopped_invocation_refs: vec![child_ref],
        }
    );
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(&parent_ref)
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );
}

#[test]
fn intrinsic_stop_selects_and_stops_work_in_the_current_conversation() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "@helper coordinate").unwrap();
    let invocation_ref = admission.invocation_ref().unwrap().to_owned();

    let response = runtime
        .stop_intrinsic_invocation(Some(admission.conversation_ref()), None, None)
        .unwrap();
    assert_eq!(
        response,
        crate::intrinsic_cwr::IntrinsicCwrResponse::InvocationStopped {
            invocation_ref: invocation_ref.clone(),
            stopped_invocation_refs: vec![invocation_ref.clone()],
        }
    );
    assert_eq!(
        runtime
            .turn(&admission.record)
            .unwrap()
            .invocation
            .unwrap()
            .state,
        ActionInvocationState::Stopped
    );

    assert_eq!(
        runtime
            .stop_intrinsic_invocation(None, Some(&invocation_ref), None)
            .unwrap(),
        crate::intrinsic_cwr::IntrinsicCwrResponse::InvocationStopTooLate { invocation_ref }
    );
}

#[test]
fn runtime_coworker_stop_can_run_ahead_of_the_turn_it_stops() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let pending = runtime.admit_text(None, "@helper coordinate").unwrap();
    let command = runtime
        .submit_text(Some(pending.conversation_ref()), "@cwr stop")
        .unwrap();
    let LocalTextOutcome::Conversation(command) = command else {
        panic!("CWR stop must use normal conversation admission")
    };

    let turn = runtime.execute_admission(&command, |_| {}).unwrap();

    assert_eq!(
        runtime
            .turn(&pending.record)
            .unwrap()
            .invocation
            .unwrap()
            .state,
        ActionInvocationState::Stopped
    );
    assert!(turn
        .conversation
        .messages
        .last()
        .unwrap()
        .text
        .starts_with("Stopped "));
}

#[test]
fn runtime_coworker_control_skips_terminal_turns_and_leaves_pending_work_queued() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let pending = runtime.admit_text(None, "@helper coordinate").unwrap();
    let help = runtime
        .submit_text(Some(pending.conversation_ref()), "@cwr help")
        .unwrap();
    let LocalTextOutcome::Conversation(help) = help else {
        panic!("CWR help must use normal conversation admission")
    };

    runtime.execute_admission(&help, |_| {}).unwrap();
    assert_eq!(
        runtime
            .turn(&pending.record)
            .unwrap()
            .invocation
            .unwrap()
            .state,
        ActionInvocationState::Admitted
    );

    let stop = runtime
        .submit_text(Some(pending.conversation_ref()), "@cwr stop")
        .unwrap();
    let LocalTextOutcome::Conversation(stop) = stop else {
        panic!("CWR stop must use normal conversation admission")
    };
    runtime.execute_admission(&stop, |_| {}).unwrap();
    assert_eq!(
        runtime
            .turn(&pending.record)
            .unwrap()
            .invocation
            .unwrap()
            .state,
        ActionInvocationState::Stopped
    );
}

#[test]
fn intrinsic_stop_reports_missing_context_and_selectors_without_mutating_work() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "@helper coordinate").unwrap();

    let response = runtime.stop_intrinsic_invocation(None, None, None).unwrap();
    assert!(matches!(
        response,
        crate::intrinsic_cwr::IntrinsicCwrResponse::InvocationStopFailed {
            invocation_ref: None,
            ref error,
        } if error.contains("provide an invocation ref")
    ));
    let response = runtime
        .stop_intrinsic_invocation(None, Some("missing"), None)
        .unwrap();
    assert!(matches!(
        response,
        crate::intrinsic_cwr::IntrinsicCwrResponse::InvocationStopFailed {
            invocation_ref: Some(ref invocation_ref),
            ref error,
        } if invocation_ref == "missing" && error.contains("not found")
    ));
    assert_eq!(
        runtime
            .turn(&admission.record)
            .unwrap()
            .invocation
            .unwrap()
            .state,
        ActionInvocationState::Admitted
    );
}

#[test]
fn stopping_a_completed_parent_stops_running_children_without_rewriting_its_reply() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), temporary.path().join("state.sqlite"), fixture_host_identity(),
    ).unwrap();
    let (parent_ref, child_ref) = detached_parent_and_child(&runtime);
    let before = {
        let mut state = runtime.lock().unwrap();
        state.admission.publish_root_message_completed(
            &parent_ref, ActionResponseStatus::COMPLETED, json!({"text":"Parent done"}), "Parent done".into(), 30,
        ).unwrap();
        assert_eq!(state.admission.invocation_record(&child_ref).unwrap().state, ActionInvocationState::Admitted);
        state.admission.invocation_record(&parent_ref).unwrap().clone()
    };
    runtime.stop_invocation(&parent_ref, Some("Stop remaining work".into())).unwrap();
    let state = runtime.lock().unwrap();
    assert_eq!(state.admission.invocation_record(&parent_ref).unwrap(), &before);
    assert_eq!(state.admission.invocation_record(&child_ref).unwrap().state, ActionInvocationState::Stopped);
}
