fn retain_adapter_checkpoint(
    store: &mut ConversationAdmissionStore,
    claimed: &ClaimedConversationInvocation,
    live_started: bool,
) {
    let adapter = &claimed.adapter_invocation;
    store
        .append_execution_activity(
            &adapter.invocation_ref,
            ConversationActivityEvent::ProviderEvent {
                provider: "codex".into(),
                method: "cwr.adapter_checkpoint".into(),
                data: json!({
                    "version": 2,
                    "invocation_ref": adapter.invocation_ref,
                    "integration_ref": adapter.integration_ref,
                    "live_started": live_started,
                }),
            },
        )
        .unwrap();
}

#[test]
fn pre_provider_stop_reuses_the_nearest_settled_history_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pre-provider-stop.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let first = working_turn(&mut store, None, "first settled request");
    let first_claim = claim_working(&mut store, &first);
    settle_working(&mut store, &first_claim);

    let stopped = working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "request stopped during preparation",
    );
    let stopped_claim = claim_working(&mut store, &stopped);
    store
        .record_invocation_progress_with_publication(
            &stopped_claim.adapter_invocation.invocation_ref,
            102,
            "Preparation progress remains public",
            true,
        )
        .unwrap();
    retain_adapter_checkpoint(&mut store, &stopped_claim, false);
    store
        .stop_invocation(
            &stopped_claim.adapter_invocation.invocation_ref,
            crate::v2026_09::invocation::InvocationStopRequest::new(103),
        )
        .unwrap();
    store
        .settle_stopped_invocation(&stopped_claim.adapter_invocation.invocation_ref, 103)
        .unwrap();
    drop(store);

    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let followup = working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "continue from settled history",
    );
    let followup_claim = claim_working(&mut store, &followup);
    let thread = &followup_claim.adapter_invocation.thread;
    assert_eq!(
        thread["working_conversation"]["ref"],
        first_claim.adapter_invocation.thread["working_conversation"]["ref"]
    );
    assert_eq!(
        thread["working_conversation"]["previous_invocation_ref"],
        first_claim.adapter_invocation.invocation_ref
    );
    assert_eq!(thread["context_start"], 1);
    for text in [
        "request stopped during preparation",
        "Preparation progress remains public",
        "continue from settled history",
    ] {
        assert!(
            thread["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["text"] == text),
            "missing public record {text}"
        );
    }
    assert_ne!(
        first.record.execution_conversation_ref,
        stopped.record.execution_conversation_ref
    );
    assert_ne!(
        stopped.record.execution_conversation_ref,
        followup.record.execution_conversation_ref
    );
}

#[test]
fn stop_after_live_provider_execution_keeps_the_history_gap() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("live-stop.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = working_turn(&mut store, None, "first settled request");
    let first_claim = claim_working(&mut store, &first);
    settle_working(&mut store, &first_claim);
    let stopped = working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "request stopped after live execution",
    );
    let stopped_claim = claim_working(&mut store, &stopped);
    retain_adapter_checkpoint(&mut store, &stopped_claim, true);
    store
        .stop_invocation(
            &stopped_claim.adapter_invocation.invocation_ref,
            crate::v2026_09::invocation::InvocationStopRequest::new(102),
        )
        .unwrap();
    store
        .settle_stopped_invocation(&stopped_claim.adapter_invocation.invocation_ref, 102)
        .unwrap();

    let followup = working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "start fresh after live stop",
    );
    let followup_claim = claim_working(&mut store, &followup);
    let working = &followup_claim.adapter_invocation.thread["working_conversation"];
    assert_eq!(
        working["ref"],
        followup_claim.execution_conversation.conversation_ref
    );
    assert_eq!(
        working["previous_invocation_ref"],
        stopped_claim.adapter_invocation.invocation_ref
    );
    assert!(working["history_gap"]
        .as_str()
        .unwrap()
        .contains("previous request was stopped"));
    assert!(working.get("state").is_none());
}
