#[test]
fn start_rejects_invalid_refs_and_action_names() {
    let mut invalid_ref = root_request("~@/invocations/invalid-ref");
    invalid_ref.target_actor_ref = "actors/parent".into();

    let mut invalid_action = root_request("~@/invocations/invalid-action");
    invalid_action.action_name = "draft-brief".into();

    for (expected_label, request) in [
        ("target actor ref", invalid_ref),
        ("action name", invalid_action),
    ] {
        let mut ledger = ActionInvocationLedger::new();
        let error = ledger.start(request).unwrap_err();
        assert!(
            error.to_string().contains(expected_label),
            "unexpected error for {expected_label}: {error}"
        );
        assert!(ledger.is_empty());
    }
}

#[test]
fn reopened_ledger_rejects_invalid_refs_and_action_names() {
    const INVOCATION_REF: &str = "~@/invocations/parent";

    let mut ledger = ActionInvocationLedger::new();
    ledger.start(root_request(INVOCATION_REF)).unwrap();
    let serialized = serde_json::to_value(&ledger).unwrap();

    for (field, invalid_value, expected_label) in [
        ("target_actor_ref", "actors/parent", "target actor ref"),
        ("action_name", "draft-brief", "action name"),
    ] {
        let mut corrupted = serialized.clone();
        corrupted["invocations"][INVOCATION_REF][field] = json!(invalid_value);
        let reopened: ActionInvocationLedger = serde_json::from_value(corrupted).unwrap();
        let error = reopened.validate().unwrap_err();
        assert!(
            error.to_string().contains(expected_label),
            "unexpected error for {expected_label}: {error}"
        );
    }
}

#[test]
fn host_call_start_is_idempotent_for_attached_and_detached_children() {
    {
        let mut ledger = ActionInvocationLedger::new();
        start_running_root(&mut ledger, "~@/invocations/parent");
        let request = child_request(
            "~@/invocations/child-first",
            "~@/invocations/parent",
            "call-1",
        );

        let StartInvocationOutcome::Started(_started) = ledger.start(request.clone()).unwrap()
        else {
            panic!("first start must create the child")
        };

        let mut replay = request;
        replay.invocation_ref = "~@/invocations/child-retry".into();
        replay.admitted_at_unix_ms = 999;
        let StartInvocationOutcome::Existing(existing) = ledger.start(replay).unwrap() else {
            panic!("same host call must reuse the first child")
        };
        assert_eq!(existing.invocation_ref, "~@/invocations/child-first");
        assert_eq!(ledger.len(), 2);
    }
}

#[test]
fn changed_host_call_replay_is_rejected() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/parent");
    let request = child_request(
        "~@/invocations/child",
        "~@/invocations/parent",
        "call-1",
    );
    ledger.start(request.clone()).unwrap();

    let mut changed = request;
    changed.invocation_ref = "~@/invocations/retry".into();
    changed.input = json!({"changed": true});
    assert!(ledger.start(changed).is_err());
    assert_eq!(ledger.len(), 2);
}

#[test]
fn ledger_round_trip_retains_the_host_call_receipt() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/parent");
    ledger
        .start(child_request(
            "~@/invocations/child",
            "~@/invocations/parent",
            "call-1",
        ))
        .unwrap();

    let bytes = serde_json::to_vec(&ledger).unwrap();
    let mut reopened: ActionInvocationLedger = serde_json::from_slice(&bytes).unwrap();
    reopened.validate().unwrap();
    let mut replay = child_request(
        "~@/invocations/retry",
        "~@/invocations/parent",
        "call-1",
    );
    replay.admitted_at_unix_ms = 777;
    let StartInvocationOutcome::Existing(existing) = reopened.start(replay).unwrap() else {
        panic!("reopen must retain the idempotency receipt")
    };
    assert_eq!(existing.invocation_ref, "~@/invocations/child");
}

#[test]
fn claim_has_one_active_winner_and_recovery_releases_the_attempt() {
    let mut ledger = ActionInvocationLedger::new();
    ledger.start(root_request("~@/invocations/parent")).unwrap();

    let ClaimInvocationOutcome::Claimed(first) = ledger.claim("~@/invocations/parent", 11).unwrap()
    else {
        panic!("first claim must win")
    };
    assert_eq!(first.invocation.attempt_count, 1);
    assert!(matches!(
        ledger.claim("~@/invocations/parent", 12).unwrap(),
        ClaimInvocationOutcome::NotRunnable {
            state: ActionInvocationState::Running,
            attempt_active: true,
        }
    ));

    ledger
        .record_progress(
            "~@/invocations/parent",
            ActionResponseStatus::PROGRESS,
            "Still working",
        )
        .unwrap();
    assert_eq!(
        ledger.recover_interrupted_attempts(),
        vec!["~@/invocations/parent".to_owned()]
    );
    assert_eq!(
        ledger.invocation("~@/invocations/parent").unwrap().state,
        ActionInvocationState::Running
    );

    let ClaimInvocationOutcome::Claimed(recovered) =
        ledger.claim("~@/invocations/parent", 14).unwrap()
    else {
        panic!("recovered running work must be claimable")
    };
    assert_eq!(recovered.invocation.attempt_count, 2);
    assert_eq!(recovered.invocation.responses.len(), 1);
}

#[test]
fn stop_rejects_an_invalid_requesting_actor_without_mutating_the_ledger() {
    const INVOCATION_REF: &str = "~@/invocations/parent";
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, INVOCATION_REF);
    let before = ledger.clone();
    let mut request = InvocationStopRequest::new(20);
    request.requested_by_actor_ref = Some("actors/requester".into());

    let error = ledger.stop(INVOCATION_REF, request).unwrap_err();

    assert!(
        error.to_string().contains("stop-requesting actor ref"),
        "unexpected error: {error}"
    );
    assert_eq!(ledger, before);

    let mut request = InvocationStopRequest::new(21);
    request.request_id = Some("   ".into());
    let error = ledger.stop(INVOCATION_REF, request).unwrap_err();
    assert!(
        error.to_string().contains("stop request id"),
        "unexpected error: {error}"
    );
    assert_eq!(ledger, before);
}

#[test]
fn validation_rejects_impossible_attempt_state_combinations_but_accepts_recovery() {
    const INVOCATION_REF: &str = "~@/invocations/parent";

    let mut admitted = ActionInvocationLedger::new();
    admitted.start(root_request(INVOCATION_REF)).unwrap();
    {
        let invocation = admitted.invocations.get_mut(INVOCATION_REF).unwrap();
        invocation.attempt_count = 1;
        invocation.started_at_unix_ms = Some(11);
    }
    let error = admitted.validate().unwrap_err().to_string();
    assert!(error.contains("admitted"), "unexpected error: {error}");

    let mut recovered = ActionInvocationLedger::new();
    start_running_root(&mut recovered, INVOCATION_REF);
    assert_eq!(
        recovered.recover_interrupted_attempts(),
        vec![INVOCATION_REF.to_owned()]
    );
    recovered.validate().unwrap();

    let impossible_running = recovered.invocations.get_mut(INVOCATION_REF).unwrap();
    impossible_running.attempt_count = 0;
    impossible_running.started_at_unix_ms = None;
    let error = recovered.validate().unwrap_err().to_string();
    assert!(error.contains("running"), "unexpected error: {error}");

    for (expected_state, mut ledger) in [
        (
            "completed",
            terminal_ledger(InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Done".into(),
                result: Some(json!({})),
                error: None,
            }),
        ),
        (
            "blocked",
            terminal_ledger(InvocationTerminal {
                status: ActionResponseStatus::BLOCKED,
                summary: "Blocked".into(),
                result: None,
                error: Some(ActionResponseDetail::new("blocked")),
            }),
        ),
        (
            "failed",
            terminal_ledger(InvocationTerminal {
                status: ActionResponseStatus::FAILED,
                summary: "Failed".into(),
                result: None,
                error: Some(ActionResponseDetail::new("failed")),
            }),
        ),
    ] {
        let invocation = ledger.invocations.get_mut(INVOCATION_REF).unwrap();
        invocation.attempt_count = 0;
        invocation.started_at_unix_ms = None;
        let error = ledger.validate().unwrap_err().to_string();
        assert!(
            error.contains(expected_state),
            "unexpected {expected_state} error: {error}"
        );
    }
}

fn terminal_ledger(terminal: InvocationTerminal) -> ActionInvocationLedger {
    const INVOCATION_REF: &str = "~@/invocations/parent";
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, INVOCATION_REF);
    ledger.finish(INVOCATION_REF, terminal, 30).unwrap();
    ledger
}


#[test]
fn admission_sequence_survives_replay_reload_and_equal_or_backward_timestamps() {
    let mut ledger = ActionInvocationLedger::new();
    start_running_root(&mut ledger, "~@/invocations/parent");
    let mut child = child_request("~@/invocations/z-child", "~@/invocations/parent", "control");
    child.admitted_at_unix_ms = 10;
    ledger.start(child.clone()).unwrap();
    ledger.start(child).unwrap();
    let mut root = root_request("~@/invocations/a-root");
    root.admitted_at_unix_ms = 1;
    ledger.start(root).unwrap();
    assert_eq!(
        ledger
            .invocation("~@/invocations/z-child")
            .unwrap()
            .admission_sequence,
        2
    );
    assert_eq!(
        ledger
            .invocation("~@/invocations/a-root")
            .unwrap()
            .admission_sequence,
        3
    );
    let mut reopened: ActionInvocationLedger =
        serde_json::from_value(serde_json::to_value(&ledger).unwrap()).unwrap();
    reopened.start(root_request("~@/invocations/next")).unwrap();
    assert_eq!(
        reopened
            .invocation("~@/invocations/next")
            .unwrap()
            .admission_sequence,
        4
    );
}
#[test]
fn timeline_publications_are_idempotent_and_allow_distinct_placements() {
    let mut ledger = ActionInvocationLedger::new();
    ledger.start(root_request("~@/invocations/root")).unwrap();
    let source = "~@/messages/source";
    let publication = "cwp://example.test/publications/root";
    let disposition = crate::v2026_09::conversation::ConversationTimelineDisposition::Root;

    ledger
        .retain_timeline_publication(
            "~@/invocations/root",
            source,
            publication,
            disposition.clone(),
        )
        .unwrap();
    ledger
        .retain_timeline_publication(
            "~@/invocations/root",
            source,
            publication,
            disposition,
        )
        .unwrap();
    let second_publication = "cwp://example.test/publications/second";
    ledger
        .retain_timeline_publication(
            "~@/invocations/root",
            source,
            second_publication,
            crate::v2026_09::conversation::ConversationTimelineDisposition::Root,
        )
        .unwrap();
    ledger.validate().unwrap();

    let retained = ledger.invocation("~@/invocations/root").unwrap();
    assert_eq!(retained.source_message_ref.as_deref(), Some(source));
    assert_eq!(retained.timeline_publications.len(), 2);
    assert_eq!(
        retained.timeline_publications.get(publication),
        Some(&crate::v2026_09::conversation::ConversationTimelineDisposition::Root)
    );
    assert_eq!(
        retained.timeline_publications.get(second_publication),
        Some(&crate::v2026_09::conversation::ConversationTimelineDisposition::Root)
    );
    assert!(ledger
        .retain_timeline_publication(
            "~@/invocations/root",
            source,
            publication,
            crate::v2026_09::conversation::ConversationTimelineDisposition::Undisclosed,
        )
        .is_err());
    for invalid in [
        "~@/publications/relative",
        "cwp://example.test/publications/fragmented#member",
    ] {
        assert!(ledger
            .retain_timeline_publication(
                "~@/invocations/root",
                source,
                invalid,
                crate::v2026_09::conversation::ConversationTimelineDisposition::Root,
            )
            .is_err());
    }
    for (index, invalid_parent) in [
        "~@/timeline/items/relative",
        "cwp://example.test/timeline/items/fragmented#member",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(ledger
            .retain_timeline_publication(
                "~@/invocations/root",
                source,
                &format!("cwp://example.test/publications/invalid-child-{index}"),
                crate::v2026_09::conversation::ConversationTimelineDisposition::Child {
                    parent_item_ref: invalid_parent.into(),
                },
            )
            .is_err());
    }
}
