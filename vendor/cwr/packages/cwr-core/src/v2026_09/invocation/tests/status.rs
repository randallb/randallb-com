#[test]
fn action_response_status_boundaries_are_exact_numeric_values() {
    for (status, expected_state) in [
        (99, None),
        (100, Some(None)),
        (199, Some(None)),
        (200, Some(Some(ActionInvocationState::Completed))),
        (299, Some(Some(ActionInvocationState::Completed))),
        (300, None),
        (399, None),
        (400, Some(Some(ActionInvocationState::Blocked))),
        (498, Some(Some(ActionInvocationState::Blocked))),
        (499, Some(Some(ActionInvocationState::Stopped))),
        (500, Some(Some(ActionInvocationState::Failed))),
        (599, Some(Some(ActionInvocationState::Failed))),
        (600, None),
    ] {
        let parsed = ActionResponseStatus::new(status);
        match expected_state {
            Some(expected_state) => {
                let parsed = parsed.unwrap_or_else(|error| panic!("{status}: {error}"));
                assert_eq!(parsed.as_u16(), status);
                assert_eq!(parsed.invocation_state(), expected_state);
                assert_eq!(serde_json::to_value(parsed).unwrap(), json!(status));
                assert_eq!(
                    serde_json::from_value::<ActionResponseStatus>(json!(status)).unwrap(),
                    parsed
                );
            }
            None => {
                assert!(parsed.is_err(), "{status} must be rejected");
                assert!(serde_json::from_value::<ActionResponseStatus>(json!(status)).is_err());
            }
        }
    }
    assert!(serde_json::from_value::<ActionResponseStatus>(json!("stopped")).is_err());
}

#[test]
fn action_response_deserialization_enforces_each_status_field_matrix() {
    let valid = [
        json!({
            "invocation_ref": "~@/invocations/one",
            "sequence": 1,
            "status": 150,
            "summary": "Working"
        }),
        json!({
            "invocation_ref": "~@/invocations/one",
            "sequence": 1,
            "status": 202,
            "summary": "Accepted",
            "result": {"job_ref": "outside-work"}
        }),
        json!({
            "invocation_ref": "~@/invocations/one",
            "sequence": 1,
            "status": 204,
            "summary": "No content",
            "result": null
        }),
        json!({
            "invocation_ref": "~@/invocations/one",
            "sequence": 1,
            "status": 451,
            "summary": "Unavailable",
            "error": {"code": "unavailable", "message": "Policy blocks access"}
        }),
        json!({
            "invocation_ref": "~@/invocations/one",
            "sequence": 1,
            "status": 499,
            "summary": "Stopped",
            "cause": {"code": "stop_requested"}
        }),
        json!({
            "invocation_ref": "~@/invocations/one",
            "sequence": 1,
            "status": 503,
            "summary": "Unavailable",
            "error": {"code": "provider_unavailable", "details": null}
        }),
    ];
    for response in valid {
        let parsed: ActionResponse = serde_json::from_value(response.clone())
            .unwrap_or_else(|error| panic!("{response}: {error}"));
        assert_eq!(serde_json::to_value(parsed).unwrap(), response);
    }

    let invalid = [
        json!({
            "invocation_ref": "~@/invocations/one", "sequence": 1,
            "status": 150, "summary": "Working", "result": {}
        }),
        json!({
            "invocation_ref": "~@/invocations/one", "sequence": 1,
            "status": 151, "summary": "Working", "result": null
        }),
        json!({
            "invocation_ref": "~@/invocations/one", "sequence": 1,
            "status": 202, "summary": "Accepted"
        }),
        json!({
            "invocation_ref": "~@/invocations/one", "sequence": 1,
            "status": 451, "summary": "Unavailable"
        }),
        json!({
            "invocation_ref": "~@/invocations/one", "sequence": 1,
            "status": 452, "summary": "Unavailable", "error": null
        }),
        json!({
            "invocation_ref": "~@/invocations/one", "sequence": 1,
            "status": 203, "summary": "Done", "result": {}, "error": null
        }),
        json!({
            "invocation_ref": "~@/invocations/one", "sequence": 1,
            "status": 499, "summary": "Stopped", "error": {"code": "wrong"}
        }),
        json!({
            "invocation_ref": "~@/invocations/one", "sequence": 1,
            "status": 503, "summary": "Unavailable", "error": {"code": " "}
        }),
        json!({
            "invocation_ref": "~@/invocations/one", "sequence": 1,
            "status": 504, "summary": "Unavailable",
            "error": {"code": "provider_unavailable", "message": null}
        }),
    ];
    for response in invalid {
        assert!(
            serde_json::from_value::<ActionResponse>(response.clone()).is_err(),
            "{response} must be rejected"
        );
    }
}

#[test]
fn status_202_is_terminal_and_status_499_is_host_only() {
    let mut completed = ActionInvocationLedger::new();
    start_running_root(&mut completed, "~@/invocations/completed");
    let FinishInvocationOutcome::Accepted { response, .. } = completed
        .finish(
            "~@/invocations/completed",
            InvocationTerminal {
                status: ActionResponseStatus::new(202).unwrap(),
                summary: "Accepted".into(),
                result: Some(json!({"job_ref": "separate-work"})),
                error: None,
            },
            20,
        )
        .unwrap()
    else {
        panic!("202 must terminalize the invocation")
    };
    assert_eq!(response.status.as_u16(), 202);
    assert_eq!(
        completed
            .invocation("~@/invocations/completed")
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );

    let mut stopped = ActionInvocationLedger::new();
    start_running_root(&mut stopped, "~@/invocations/stopped");
    assert!(stopped
        .finish(
            "~@/invocations/stopped",
            InvocationTerminal {
                status: ActionResponseStatus::STOPPED,
                summary: "Adapter tried to stop".into(),
                result: None,
                error: None,
            },
            20,
        )
        .is_err());
    let StopInvocationOutcome::Accepted { response, .. } = stopped
        .stop("~@/invocations/stopped", InvocationStopRequest::new(21))
        .unwrap()
    else {
        panic!("host stop must be accepted")
    };
    assert_eq!(response.status, ActionResponseStatus::STOPPED);
}

#[test]
fn child_results_preserve_exact_status_while_parent_stays_live() {
    for status in [202, 451, 499, 503] {
        let parent_ref = format!("~@/invocations/parent-{status}");
        let child_ref = format!("~@/invocations/child-{status}");
        let mut ledger = ActionInvocationLedger::new();
        start_running_root(&mut ledger, &parent_ref);
        ledger
            .start(child_request(
                &child_ref,
                &parent_ref,
                "start-child",
            ))
            .unwrap();
        ledger.claim(&child_ref, 20).unwrap();
        let response = match status {
            202 => {
                let FinishInvocationOutcome::Accepted { response, .. } = ledger
                    .finish(
                        &child_ref,
                        InvocationTerminal {
                            status: ActionResponseStatus::new(status).unwrap(),
                            summary: "Accepted".into(),
                            result: Some(Value::Null),
                            error: None,
                        },
                        30,
                    )
                    .unwrap()
                else {
                    panic!("child completion must be accepted")
                };
                response
            }
            451 | 503 => {
                let FinishInvocationOutcome::Accepted { response, .. } = ledger
                    .finish(
                        &child_ref,
                        InvocationTerminal {
                            status: ActionResponseStatus::new(status).unwrap(),
                            summary: "Unavailable".into(),
                            result: None,
                            error: Some(
                                ActionResponseDetail::new("unavailable").with_details(Value::Null),
                            ),
                        },
                        30,
                    )
                    .unwrap()
                else {
                    panic!("child error must be accepted")
                };
                response
            }
            499 => {
                let StopInvocationOutcome::Accepted { response, .. } = ledger
                    .stop(&child_ref, InvocationStopRequest::new(30))
                    .unwrap()
                else {
                    panic!("child stop must be accepted")
                };
                response
            }
            _ => unreachable!(),
        };

        assert!(ledger.invocation(&parent_ref).unwrap().attempt_active);
        assert_eq!(ledger.invocation(&parent_ref).unwrap().attempt_count, 1);
        assert_eq!(ledger.invocation(&child_ref).unwrap().terminal_response(), Some(&response));
        assert!(ledger.waits().is_empty());
    }
}
