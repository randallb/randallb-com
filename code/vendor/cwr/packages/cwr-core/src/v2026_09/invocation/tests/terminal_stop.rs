#[test]
fn first_terminal_response_wins() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/parent");

    let FinishInvocationOutcome::Accepted { response, .. } = ledger
        .finish(
            "~@/invocations/parent",
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Done".into(),
                result: Some(json!({"answer": 42})),
                error: None,
            },
            30,
        )
        .unwrap()
    else {
        panic!("completion must win")
    };
    assert_eq!(response.status, ActionResponseStatus::COMPLETED);

    let FinishInvocationOutcome::AlreadyTerminal { response } = ledger
        .finish(
            "~@/invocations/parent",
            InvocationTerminal {
                status: ActionResponseStatus::FAILED,
                summary: "Late failure".into(),
                result: None,
                error: Some(ActionResponseDetail::new("late_failure")),
            },
            31,
        )
        .unwrap()
    else {
        panic!("late terminal must lose")
    };
    assert_eq!(response.status, ActionResponseStatus::COMPLETED);
    assert!(matches!(
        ledger
            .stop(
                "~@/invocations/parent",
                InvocationStopRequest::new(32)
            )
            .unwrap(),
        StopInvocationOutcome::TooLate { response }
            if response.status == ActionResponseStatus::COMPLETED
    ));
    assert_eq!(
        ledger
            .invocation("~@/invocations/parent")
            .unwrap()
            .responses
            .len(),
        1
    );
}

#[test]
fn stopped_records_require_actor_integration_only_after_execution_starts() {
    const PRE_EXECUTION_REF: &str = "~@/invocations/pre-execution-stop";
    let mut request = root_request(PRE_EXECUTION_REF);
    request.actor_integration_ref = None;
    let mut pre_execution = ActionInvocationLedger::new();
    pre_execution.start(request).unwrap();
    pre_execution
        .stop(PRE_EXECUTION_REF, InvocationStopRequest::new(20))
        .unwrap();
    pre_execution.validate().unwrap();

    const STARTED_REF: &str = "~@/invocations/started-stop";
    let mut started = ActionInvocationLedger::new();
    start_running_root(&mut started, STARTED_REF);
    started
        .stop(STARTED_REF, InvocationStopRequest::new(20))
        .unwrap();
    started
        .invocations
        .get_mut(STARTED_REF)
        .unwrap()
        .actor_integration_ref = None;
    let error = started.validate().unwrap_err().to_string();
    assert!(
        error.contains("actor integration"),
        "unexpected error: {error}"
    );

    let mut progressed = pre_execution;
    let invocation = progressed.invocations.get_mut(PRE_EXECUTION_REF).unwrap();
    invocation.responses[0].sequence = 2;
    invocation.responses.insert(
        0,
        ActionResponse {
            progress: None,
            invocation_ref: PRE_EXECUTION_REF.into(),
            sequence: 1,
            status: ActionResponseStatus::PROGRESS,
            summary: "Started work".into(),
            result: None,
            error: None,
            cause: None,
        },
    );
    let error = progressed.validate().unwrap_err().to_string();
    assert!(
        error.contains("actor integration"),
        "unexpected error: {error}"
    );
}

#[test]
fn active_stop_has_no_finish_time_until_adapter_cancellation_settles() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/root");
    ledger
        .stop("~@/invocations/root", InvocationStopRequest::new(20))
        .unwrap();

    let pending = ledger.invocation("~@/invocations/root").unwrap();
    assert_eq!(pending.state, ActionInvocationState::Stopping);
    assert_eq!(pending.finished_at_unix_ms, None);
    ledger.validate().unwrap();
    assert!(!ledger.settle_stopped("~@/invocations/root", 35).unwrap().is_empty());
    let record = ledger.invocation("~@/invocations/root").unwrap();
    assert_eq!(record.stop_requested_at_unix_ms, Some(20));
    assert_eq!(record.finished_at_unix_ms, Some(35));
    assert!(ledger.settle_stopped("~@/invocations/root", 40).unwrap().is_empty());
}

#[test]
fn stop_propagates_through_all_descendants() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/parent");
    ledger
        .start(child_request(
            "~@/invocations/attached",
            "~@/invocations/parent",
            "call-attached",
        ))
        .unwrap();
    ledger
        .start(child_request(
            "~@/invocations/detached",
            "~@/invocations/parent",
            "call-detached",
        ))
        .unwrap();
    ledger.claim("~@/invocations/attached", 21).unwrap();
    ledger
        .start(child_request(
            "~@/invocations/grandchild",
            "~@/invocations/attached",
            "call-grandchild",
        ))
        .unwrap();

    let StopInvocationOutcome::Accepted {
        stopped_invocation_refs,
        ..
    } = ledger
        .stop(
            "~@/invocations/parent",
            InvocationStopRequest {
                requested_at_unix_ms: 40,
                requested_by_actor_ref: Some("~@/actors/requester".into()),
                request_id: Some("stop-call-1".into()),
                reason: Some("No longer needed".into()),
            },
        )
        .unwrap()
    else {
        panic!("stop must be accepted")
    };
    assert_eq!(
        stopped_invocation_refs,
        vec![
            "~@/invocations/parent".to_owned(),
            "~@/invocations/attached".to_owned(),
            "~@/invocations/grandchild".to_owned(),
            "~@/invocations/detached".to_owned(),
        ]
    );
    for invocation_ref in &stopped_invocation_refs {
        let stopped = ledger.invocation(invocation_ref).unwrap();
        assert_eq!(
            stopped.state,
            if matches!(
                invocation_ref.as_str(),
                "~@/invocations/parent" | "~@/invocations/attached"
            ) {
                ActionInvocationState::Stopping
            } else {
                ActionInvocationState::Stopped
            }
        );
        assert_eq!(stopped.stop_request_id.as_deref(), Some("stop-call-1"));
        assert_eq!(
            stopped.responses.last().unwrap().status,
            ActionResponseStatus::STOPPED
        );
    }
    assert_eq!(
        ledger.invocation("~@/invocations/detached").unwrap().state,
        ActionInvocationState::Stopped
    );
    let reopened: ActionInvocationLedger =
        serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
    reopened.validate().unwrap();
    assert_eq!(
        reopened
            .invocation("~@/invocations/grandchild")
            .unwrap()
            .stop_request_id
            .as_deref(),
        Some("stop-call-1")
    );

    let mut corrupted = reopened;
    corrupted
        .invocations
        .get_mut("~@/invocations/attached")
        .unwrap()
        .stop_request_id = Some(" ".into());
    let error = corrupted.validate().unwrap_err().to_string();
    assert!(
        error.contains("stop request id"),
        "unexpected error: {error}"
    );
}

#[test]
fn stop_preserves_an_attached_child_that_already_completed() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/parent");
    ledger
        .start(child_request(
            "~@/invocations/completed-child",
            "~@/invocations/parent",
            "call-completed-child",
        ))
        .unwrap();
    ledger
        .claim("~@/invocations/completed-child", 21)
        .unwrap();
    ledger
        .finish(
            "~@/invocations/completed-child",
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Child done".into(),
                result: Some(json!({"done": true})),
                error: None,
            },
            22,
        )
        .unwrap();

    let StopInvocationOutcome::Accepted {
        stopped_invocation_refs,
        ..
    } = ledger
        .stop("~@/invocations/parent", InvocationStopRequest::new(30))
        .unwrap()
    else {
        panic!("stop must be accepted")
    };

    assert_eq!(stopped_invocation_refs, ["~@/invocations/parent"]);
    assert_eq!(
        ledger
            .invocation("~@/invocations/completed-child")
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );
    ledger.validate().unwrap();
}

#[test]
fn identical_stop_request_replays_the_accepted_subtree_without_new_responses() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/parent");
    ledger
        .start(child_request(
            "~@/invocations/attached",
            "~@/invocations/parent",
            "call-attached-replay",
        ))
        .unwrap();
    ledger
        .start(child_request(
            "~@/invocations/detached",
            "~@/invocations/parent",
            "call-detached-replay",
        ))
        .unwrap();
    let request = InvocationStopRequest {
        requested_at_unix_ms: 40,
        requested_by_actor_ref: Some("~@/actors/requester".into()),
        request_id: Some("stop-replay-1".into()),
        reason: Some("No longer needed".into()),
    };

    let StopInvocationOutcome::Accepted {
        response,
        stopped_invocation_refs,
        replayed,
        ..
    } = ledger
        .stop("~@/invocations/parent", request.clone())
        .unwrap()
    else {
        panic!("first stop must be accepted")
    };
    assert!(!replayed);
    assert_eq!(
        stopped_invocation_refs,
        [
            "~@/invocations/parent".to_owned(),
            "~@/invocations/attached".to_owned(),
            "~@/invocations/detached".to_owned(),
        ]
    );
    let response_counts = stopped_invocation_refs
        .iter()
        .map(|invocation_ref| {
            (
                invocation_ref.clone(),
                ledger.invocation(invocation_ref).unwrap().responses.len(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let StopInvocationOutcome::Accepted {
        response: replayed_response,
        stopped_invocation_refs: replayed_refs,
        woken_invocation_refs,
        replayed,
    } = ledger
        .stop("~@/invocations/parent", request.clone())
        .unwrap()
    else {
        panic!("identical stop must replay as accepted")
    };
    assert!(replayed);
    assert_eq!(replayed_response, response);
    assert_eq!(replayed_refs, stopped_invocation_refs);
    assert!(woken_invocation_refs.is_empty());
    for (invocation_ref, response_count) in response_counts {
        assert_eq!(
            ledger.invocation(&invocation_ref).unwrap().responses.len(),
            response_count,
            "replay appended another terminal response for {invocation_ref}"
        );
    }
    assert_eq!(
        ledger.invocation("~@/invocations/detached").unwrap().state,
        ActionInvocationState::Stopped
    );

    let mut different_request = request;
    different_request.request_id = Some("stop-replay-2".into());
    assert!(matches!(
        ledger
            .stop("~@/invocations/parent", different_request)
            .unwrap(),
        StopInvocationOutcome::Accepted { replayed: true, .. }
    ));
}

#[test]
fn cancellation_can_settle_in_the_same_millisecond() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/root");
    ledger
        .stop("~@/invocations/root", InvocationStopRequest::new(20))
        .unwrap();
    assert!(!ledger.settle_stopped("~@/invocations/root", 20).unwrap().is_empty());
    let settled = ledger.invocation("~@/invocations/root").unwrap();
    assert_eq!(settled.state, ActionInvocationState::Stopped);
    assert_eq!(settled.finished_at_unix_ms, Some(20));
    ledger.validate().unwrap();
}

#[test]
fn root_waits_for_its_own_executor_and_stopping_descendants() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/root");
    ledger
        .start(child_request(
            "~@/invocations/child",
            "~@/invocations/root",
            "child",
        ))
        .unwrap();
    ledger.claim("~@/invocations/child", 15).unwrap();
    ledger
        .stop("~@/invocations/root", InvocationStopRequest::new(20))
        .unwrap();
    assert!(ledger.settle_stopped("~@/invocations/root", 25).unwrap().is_empty());
    assert_eq!(
        ledger.invocation("~@/invocations/root").unwrap().state,
        ActionInvocationState::Stopping
    );
    assert_eq!(ledger.settle_stopped("~@/invocations/child", 30).unwrap(),
        vec!["~@/invocations/child", "~@/invocations/root"]);
    for reference in ["~@/invocations/root", "~@/invocations/child"] {
        let record = ledger.invocation(reference).unwrap();
        assert_eq!(record.state, ActionInvocationState::Stopped);
        assert_eq!(record.finished_at_unix_ms, Some(30));
    }
    ledger.validate().unwrap();
}

#[test]
fn repeating_a_stop_without_a_request_id_preserves_pending_settlement() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/pending");
    ledger
        .stop("~@/invocations/pending", InvocationStopRequest::new(20))
        .unwrap();
    assert!(matches!(
        ledger
            .stop("~@/invocations/pending", InvocationStopRequest::new(21))
            .unwrap(),
        StopInvocationOutcome::Accepted { replayed: true, .. }
    ));
    assert_eq!(
        ledger
            .invocation("~@/invocations/pending")
            .unwrap()
            .stop_requested_at_unix_ms,
        Some(20)
    );
    ledger.validate().unwrap();
}

#[test]
fn stop_settlement_waits_for_descendants_below_completed_children() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/root");
    ledger.start(child_request("~@/invocations/child", "~@/invocations/root", "child")).unwrap();
    ledger.claim("~@/invocations/child", 21).unwrap();
    let mut grandchild = child_request("~@/invocations/grandchild", "~@/invocations/child", "grandchild");
    grandchild.requester_actor_ref = "~@/actors/child".into();
    ledger.start(grandchild).unwrap();
    ledger.claim("~@/invocations/grandchild", 22).unwrap();
    ledger.finish("~@/invocations/child", InvocationTerminal { status: ActionResponseStatus::COMPLETED, summary: "Done".into(), result: Some(json!({})), error: None }, 23).unwrap();
    ledger.stop("~@/invocations/root", InvocationStopRequest::new(24)).unwrap();
    ledger.settle_stopped("~@/invocations/root", 25).unwrap();
    assert_eq!(ledger.invocation("~@/invocations/root").unwrap().state, ActionInvocationState::Stopping);
    ledger.settle_stopped("~@/invocations/grandchild", 30).unwrap();
    assert_eq!(ledger.invocation("~@/invocations/root").unwrap().finished_at_unix_ms, Some(30));
    assert_eq!(ledger.invocation("~@/invocations/child").unwrap().state, ActionInvocationState::Completed);
    ledger.validate().unwrap();
}
