use super::*;

#[test]
fn deferred_status_keeps_completed_execution_of_active_root_and_unrelated_history_cold() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("status.sqlite");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        "cwp://com.example.runtime",
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let completed = working_turn(&mut store, None, "completed-first");
    let root = completed.record.conversation_ref.clone();
    let completed_execution = store
        .execution_conversation_ref(completed.record.invocation_ref.as_deref().unwrap())
        .unwrap()
        .to_owned();
    let claimed = claim_working(&mut store, &completed);
    settle_working(&mut store, &claimed);
    let active = working_turn(&mut store, Some(&root), "pending-second");
    let active_execution = store
        .execution_conversation_ref(active.record.invocation_ref.as_deref().unwrap())
        .unwrap()
        .to_owned();
    let unrelated = store
        .record_host_exchange(
            None,
            crate::v2026_09::test_fixture::actor_ref("person"),
            crate::v2026_09::test_fixture::actor_ref("helper"),
            "Unrelated".into(),
            "Reply".into(),
        )
        .unwrap()
        .conversation
        .conversation_ref;
    let expected = crate::v2026_09::host::project_conversation_turn_statuses(&store);
    assert!(!expected.is_empty());
    drop(store);

    let store = ConversationAdmissionStore::open_with_timeline_rollout_deferred(
        &path,
        "cwp://com.example.runtime",
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    assert!(store.state.cold_conversation_refs.contains(&unrelated));
    for hot in [&root, &completed_execution, &active_execution] {
        assert!(!store.state.cold_conversation_refs.contains(hot));
    }
    assert_eq!(
        crate::v2026_09::host::project_conversation_turn_statuses(&store),
        expected
    );
}
