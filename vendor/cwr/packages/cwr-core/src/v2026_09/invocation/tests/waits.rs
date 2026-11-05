#[test]
fn synchronous_child_completion_keeps_the_original_parent_attempt() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/parent");
    ledger.start(child_request("~@/invocations/child", "~@/invocations/parent", "call-child")).unwrap();
    ledger.claim("~@/invocations/child", 20).unwrap();
    let FinishInvocationOutcome::Accepted { woken_invocation_refs, .. } = ledger.finish(
        "~@/invocations/child", InvocationTerminal {
            status: ActionResponseStatus::COMPLETED, summary: "Done".into(),
            result: Some(json!({"ok": true})), error: None,
        }, 21).unwrap() else { panic!("child settles") };
    assert!(woken_invocation_refs.is_empty());
    let parent = ledger.invocation("~@/invocations/parent").unwrap();
    assert_eq!(parent.state, ActionInvocationState::Running);
    assert!(parent.attempt_active);
    assert_eq!(parent.attempt_count, 1);
    assert!(ledger.waits().is_empty());
}

#[test]
fn legacy_pending_and_ready_suspensions_settle_without_replaying_children() {
    for child_completed in [false, true] {
        let mut ledger = ActionInvocationLedger::new();
        start_running_root(&mut ledger, "~@/invocations/parent");
        ledger.start(child_request("~@/invocations/child", "~@/invocations/parent", "call-child")).unwrap();
        ledger.claim("~@/invocations/child", 20).unwrap();
        if child_completed {
            ledger.finish("~@/invocations/child", InvocationTerminal {
                status: ActionResponseStatus::COMPLETED, summary: "Done".into(),
                result: Some(json!({"artifact":"retained"})), error: None,
            }, 21).unwrap();
        }
        let child = ledger.invocation("~@/invocations/child").unwrap().clone();
        let parent = ledger.invocations.get_mut("~@/invocations/parent").unwrap();
        parent.state = if child_completed { ActionInvocationState::Running } else { ActionInvocationState::Waiting };
        parent.attempt_active = false;
        ledger.waits.push(InvocationWait {
            waiter_invocation_ref: "~@/invocations/parent".into(),
            awaited_invocation_ref: "~@/invocations/child".into(),
            call_id: "call-await".into(),
            state: if child_completed { InvocationWaitState::Ready } else { InvocationWaitState::Pending },
            response: child.terminal_response().cloned(),
            resume_after_activity_sequence: None,
        });
        ledger.validate().unwrap();
        let mut reopened: ActionInvocationLedger = serde_json::from_value(serde_json::to_value(ledger).unwrap()).unwrap();
        let responses = reopened.settle_legacy_suspended(30).unwrap();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].error.as_ref().unwrap().code, "suspended_execution_unsupported");
        assert_eq!(reopened.invocation("~@/invocations/child"), Some(&child));
        assert_eq!(reopened.invocation("~@/invocations/parent").unwrap().attempt_count, 1);
        assert_eq!(reopened.waits()[0].state, InvocationWaitState::Abandoned);
        assert!(reopened.settle_legacy_suspended(31).unwrap().is_empty());
        reopened.validate().unwrap();
    }
}

#[test]
fn named_recovery_releases_only_the_selected_attempt() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/first");
    start_running_root(&mut ledger, "~@/invocations/second");
    assert!(ledger.recover_interrupted_attempt("~@/invocations/first").unwrap());
    assert!(!ledger.invocation("~@/invocations/first").unwrap().attempt_active);
    assert!(ledger.invocation("~@/invocations/second").unwrap().attempt_active);
    assert!(!ledger.recover_interrupted_attempt("~@/invocations/first").unwrap());
}
