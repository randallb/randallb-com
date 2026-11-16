fn provider_stop_request(at: u64) -> InvocationStopRequest {
    InvocationStopRequest {
        requested_at_unix_ms: at,
        requested_by_actor_ref: Some("cwp://com.example.fixture/actors/requester".into()),
        request_id: Some("slack-stop-event".into()),
        reason: None,
    }
}

#[test]
fn provider_stop_selects_the_running_turn_at_click_time_not_delivery_time() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = open(&temporary.path().join("conversation.json"));
    let next_ref = "cwp://com.example.runtime/invocations/next";
    let ledger = &mut store.state.invocation_ledger;
    ledger
        .start(selector_root(ROOT_INVOCATION_REF, CONVERSATION_REF, 10))
        .unwrap();
    ledger.claim(ROOT_INVOCATION_REF, 11).unwrap();
    ledger
        .start(selector_root(next_ref, CONVERSATION_REF, 12))
        .unwrap();
    ledger
        .finish(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Finished before delivery".into(),
                result: Some(json!({})),
                error: None,
            },
            20,
        )
        .unwrap();
    ledger.claim(next_ref, 21).unwrap();

    let (selected, request) = store
        .plan_conversation_stop(CONVERSATION_REF, provider_stop_request(30), Some(15))
        .unwrap()
        .unwrap();
    assert_eq!(selected, ROOT_INVOCATION_REF);
    assert_eq!(request.requested_at_unix_ms, 30, "do not backdate the stop");
    assert!(matches!(
        store
            .state
            .invocation_ledger
            .stop(&selected, request)
            .unwrap(),
        StopInvocationOutcome::TooLate { .. }
    ));
    assert_eq!(
        store.invocation_record(next_ref).unwrap().state,
        ActionInvocationState::Running
    );
}

#[test]
fn provider_stop_does_not_select_work_admitted_after_the_click() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = open(&temporary.path().join("conversation.json"));
    store
        .state
        .invocation_ledger
        .start(selector_root(ROOT_INVOCATION_REF, CONVERSATION_REF, 20))
        .unwrap();

    assert!(store
        .plan_conversation_stop(CONVERSATION_REF, provider_stop_request(30), Some(15))
        .unwrap()
        .is_none());
    assert!(store
        .plan_conversation_stop(
            "cwp://com.example.runtime/conversations/other",
            provider_stop_request(30),
            Some(25)
        )
        .unwrap()
        .is_none());
}

#[test]
fn provider_stop_refuses_ambiguous_turns_and_unauthorized_requesters() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = open(&temporary.path().join("conversation.json"));
    store
        .state
        .invocation_ledger
        .start(selector_root(ROOT_INVOCATION_REF, CONVERSATION_REF, 10))
        .unwrap();

    let mut outsider = provider_stop_request(30);
    outsider.requested_by_actor_ref = Some("cwp://com.example.fixture/actors/observer".into());
    assert!(store
        .plan_conversation_stop(CONVERSATION_REF, outsider, Some(15))
        .unwrap_err()
        .to_string()
        .contains("you may only stop work"));

    let mut target = provider_stop_request(30);
    target.requested_by_actor_ref = Some("cwp://com.example.fixture/actors/worker".into());
    assert!(store
        .plan_conversation_stop(CONVERSATION_REF, target, Some(15))
        .unwrap()
        .is_some());

    store
        .state
        .invocation_ledger
        .start(selector_root(
            "cwp://com.example.runtime/invocations/another",
            CONVERSATION_REF,
            12,
        ))
        .unwrap();
    assert!(store
        .plan_conversation_stop(CONVERSATION_REF, provider_stop_request(30), Some(15))
        .unwrap_err()
        .to_string()
        .contains("more than one invocation"));
    assert!(store
        .invocation_records()
        .all(|record| record.state == ActionInvocationState::Admitted));
}

#[test]
fn provider_stop_reuses_its_original_target_after_reopening_the_store() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    let (selected, request) = store
        .plan_conversation_stop(CONVERSATION_REF, provider_stop_request(30), Some(15))
        .unwrap()
        .unwrap();
    store
        .publish_root_stop(&selected, request, "Stopped.".into())
        .unwrap();
    drop(store);

    let mut store = open(&path);
    let next_ref = "cwp://com.example.runtime/invocations/next";
    store
        .state
        .invocation_ledger
        .start(selector_root(next_ref, CONVERSATION_REF, 40))
        .unwrap();
    let (selected, request) = store
        .plan_conversation_stop(CONVERSATION_REF, provider_stop_request(50), None)
        .unwrap()
        .unwrap();
    assert_eq!(selected, ROOT_INVOCATION_REF);
    assert_eq!(request.requested_at_unix_ms, 30);
    assert!(matches!(
        store
            .publish_root_stop(&selected, request, "Stopped.".into())
            .unwrap(),
        PublishInvocationStopOutcome::Accepted { .. }
    ));
    assert_eq!(
        store.invocation_record(next_ref).unwrap().state,
        ActionInvocationState::Admitted
    );

    let mut reused = provider_stop_request(50);
    reused.requested_by_actor_ref = Some("cwp://com.example.fixture/actors/worker".into());
    assert!(store
        .plan_conversation_stop(CONVERSATION_REF, reused, Some(15))
        .unwrap_err()
        .to_string()
        .contains("reused with different parameters"));
    assert!(store
        .plan_conversation_stop(
            "cwp://com.example.runtime/conversations/other",
            provider_stop_request(50),
            Some(15)
        )
        .unwrap_err()
        .to_string()
        .contains("reused with different parameters"));
}
