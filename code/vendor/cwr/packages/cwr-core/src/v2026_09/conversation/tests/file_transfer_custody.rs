#[test]
fn transfers_reject_file_refs_without_copying_custody() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.sqlite");
    let graph = fixture_graph_with_reviewer();
    let owner = crate::v2026_09::test_fixture::actor_ref("person");
    let importer = "cwp://com.example.fixture/actors/reviewer";
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let file = store.ingest_file(owner, "private.txt", "text/plain", b"private bytes").unwrap();
    let input = ingress("transfer-source", owner, None, ConversationIngressAddressing::Unaddressed, "Source");
    let ConversationAdmissionOutcome::Started(turn) = store.admit(&graph, &input, None).unwrap() else { panic!("new source") };
    let mut forged = store.export_conversation(&turn.conversation.conversation_ref, owner).unwrap();
    forged.conversation.messages[0].file_refs.push(file.file_ref.clone());
    let before = store.conversations().count();
    assert!(store.read_file(importer, &file.file_ref).is_err());
    assert!(store.import_conversation(&graph, importer, forged, Some("forged-files")).unwrap_err().to_string().contains("custody"));
    assert_eq!(store.conversations().count(), before);
    assert!(store.read_file(importer, &file.file_ref).is_err());
    let mut attached = ingress("real-attachment", owner, None, ConversationIngressAddressing::Unaddressed, "Attached");
    attached.file_refs.push(file.file_ref.clone());
    let ConversationAdmissionOutcome::Started(turn) = store.admit(&graph, &attached, None).unwrap() else { panic!("new attachment") };
    assert!(store.export_conversation(&turn.conversation.conversation_ref, owner).unwrap_err().to_string().contains("custody"));
    assert!(store.fork_conversation(&graph, owner, &turn.conversation.conversation_ref, &turn.record.source_message_ref, "file-fork").unwrap_err().to_string().contains("custody"));
    drop(store);
    let store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    assert!(store.read_file(importer, &file.file_ref).is_err());
    assert_eq!(hex::decode(store.read_file(owner, &file.file_ref).unwrap().content_hex).unwrap(), b"private bytes");
}
