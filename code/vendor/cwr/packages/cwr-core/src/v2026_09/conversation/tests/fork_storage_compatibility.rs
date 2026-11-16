fn assert_backward_readable_storage(path: &std::path::Path) { cwpdb_store::test_support::timeline_persistence::assert_backward_readable_fork(path).unwrap(); }

#[test]
fn fork_storage_stays_legacy_readable_after_continuation() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("fork-continuation.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let (source_ref, _, selected_ref, _) = create_fork_source(&mut store, &graph, person);
    let fork = store
        .fork_conversation(&graph, person, &source_ref, &selected_ref, "rollback-check")
        .unwrap();
    let fork_ref = fork.conversation_ref;
    drop(store);

    assert_backward_readable_storage(&path);
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    assert!(matches!(
        store
            .admit(
                &graph,
                &ingress(
                    "after-rollback",
                    person,
                    Some(&fork_ref),
                    ConversationIngressAddressing::Unaddressed,
                    "Continue the fork",
                ),
                None,
            )
            .unwrap(),
        ConversationAdmissionOutcome::Started(_)
    ));
    assert!(store
        .conversation(&fork_ref)
        .unwrap()
        .messages
        .iter()
        .any(|message| message.text == "Continue the fork"));
    drop(store);
    assert_backward_readable_storage(&path);
}

