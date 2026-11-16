#[test]
fn lifecycle_mutations_preserve_unrelated_history_allocations() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = open(&temporary.path().join("runtime.sqlite"));
    let mut history = Vec::new();
    for _ in 0..16 {
        let exchange = store
            .record_host_exchange(
                None,
                "cwp://com.example.fixture/actors/person",
                "cwp://com.example.fixture/actors/host",
                "request".repeat(32_000),
                "response".repeat(32_000),
            )
            .unwrap();
        let reference = exchange.conversation.conversation_ref;
        let allocation = store.conversation(&reference).unwrap().messages[0]
            .text
            .as_ptr() as usize;
        history.push((reference, allocation));
    }
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
        .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 102, "Working")
        .unwrap();
    store
        .publish_root_failure(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::FAILED,
                summary: "Expected test failure".into(),
                result: None,
                error: Some(ActionResponseDetail::new("synthetic_failure")),
            },
            "Expected test failure".into(),
            12,
        )
        .unwrap();
    for (reference, allocation) in history {
        assert_eq!(
            store.conversation(&reference).unwrap().messages[0]
                .text
                .as_ptr() as usize,
            allocation,
            "an unrelated conversation body was copied during another turn"
        );
    }
}

#[test]
fn failed_sqlite_commit_rolls_back_all_touched_records_and_can_retry() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("runtime.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let before = serde_json::to_value(&store.state).unwrap();
    cwpdb_store::test_support::invocation_reads::reject_invocation_updates(&path, true).unwrap();
    assert!(
        store
            .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 102, "Working")
            .is_err()
    );
    assert_eq!(serde_json::to_value(&store.state).unwrap(), before);
    cwpdb_store::test_support::invocation_reads::reject_invocation_updates(&path, false).unwrap();
    assert_eq!(serde_json::to_value(&open(&path).state).unwrap(), before);
    store
        .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 102, "Working")
        .unwrap();
    let after = serde_json::to_value(&store.state).unwrap();
    assert_eq!(serde_json::to_value(&open(&path).state).unwrap(), after);
    assert_eq!(
        store
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .responses
            .len(),
        1
    );
}
