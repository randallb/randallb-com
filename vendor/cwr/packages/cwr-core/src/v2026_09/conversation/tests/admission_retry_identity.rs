#[test]
fn admission_replay_requires_the_same_ingress() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let state_path = temporary.path().join("conversation.json");
    let graph = fixture_graph_with_reviewer();
    let original = ingress(
        "event-replay",
        crate::v2026_09::test_fixture::actor_ref("person"),
        None,
        ConversationIngressAddressing::Unaddressed,
        "Original message",
    );
    let mut store = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
        .expect("open store");
    let first = store
        .admit(&graph, &original, None)
        .expect("admit original ingress");
    assert!(matches!(first, ConversationAdmissionOutcome::Started(_)));
    let replay = store
        .admit(&graph, &original, None)
        .expect("replay identical ingress");
    assert!(matches!(replay, ConversationAdmissionOutcome::Existing(_)));

    let mut changed = original.clone();
    changed.text = "Different message".into();
    let error = store
        .admit(&graph, &changed, None)
        .expect_err("changed ingress with the same event ID must fail");
    assert!(error.to_string().contains("reused with different input"));

    let mut reopened = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
        .expect("reopen store");
    let error = reopened
        .admit(&graph, &changed, None)
        .expect_err("persisted fingerprint must reject changed ingress");
    assert!(error.to_string().contains("reused with different input"));
}

#[test]
fn legacy_admission_replay_rejects_changed_retained_message_input() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let graph = fixture_graph_with_reviewer();
    let original = ingress(
        "event-legacy-replay",
        crate::v2026_09::test_fixture::actor_ref("person"),
        None,
        ConversationIngressAddressing::Unaddressed,
        "Original message",
    );
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("conversation.json"),
        "cwp://com.example.runtime",
    )
    .expect("open store");
    store
        .admit(&graph, &original, None)
        .expect("admit original ingress");
    store.state.admissions[0].ingress_fingerprint = None;

    let replay = store
        .admit(&graph, &original, None)
        .expect("replay matching legacy ingress");
    assert!(matches!(replay, ConversationAdmissionOutcome::Existing(_)));
    let file = store
        .ingest_file(
            &original.author_actor_ref,
            "extra.txt",
            "text/plain",
            b"extra",
        )
        .unwrap();
    let mut changed = original.clone();
    changed.file_refs.push(file.file_ref);
    let error = store
        .admit(&graph, &changed, None)
        .expect_err("legacy replay with added attachment must fail");
    assert!(error.to_string().contains("reused with different input"));
    let mut changed = original;
    changed.text = "Different message".into();
    let error = store
        .admit(&graph, &changed, None)
        .expect_err("legacy replay with changed text must fail");
    assert!(error.to_string().contains("reused with different input"));
}

#[test]
fn legacy_admission_rejects_an_unverifiable_source_input_retry() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let graph = fixture_graph_with_reviewer();
    let original = ingress(
        "event-legacy-source-input",
        crate::v2026_09::test_fixture::actor_ref("person"),
        None,
        ConversationIngressAddressing::Unaddressed,
        "Original message",
    );
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("conversation.json"),
        "cwp://com.example.runtime",
    )
    .expect("open store");
    store
        .admit_host_local(&graph, &original, None, &[], None, None)
        .expect("admit legacy source input");

    let error = store
        .admit_host_local(
            &graph,
            &original,
            None,
            &[],
            None,
            Some("sha256:new-source-input"),
        )
        .expect_err("legacy source input cannot verify an ordinary retry");
    assert!(error.to_string().contains("has no retained source input"));
}


#[test]
fn file_links_are_admitted_atomically_and_retries_keep_exact_refs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.db");
    let graph = fixture_graph_with_reviewer();
    let author = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let file = store
        .ingest_file(author, "photo.png", "image/png", b"retained image")
        .unwrap();
    let mut input = ingress(
        "image-message",
        author,
        None,
        ConversationIngressAddressing::Unaddressed,
        "",
    );
    input.file_refs.push(file.file_ref.clone());
    store.admit(&graph, &input, None).unwrap();
    assert!(matches!(
        store.admit(&graph, &input, None).unwrap(),
        ConversationAdmissionOutcome::Existing(_)
    ));
    input.file_refs.clear();
    input.text = "different".into();
    assert!(store.admit(&graph, &input, None).is_err());
    drop(store);
    let store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    assert_eq!(
        store.read_file(author, &file.file_ref).unwrap().metadata,
        file
    );
}
