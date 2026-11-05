#[test]
fn no_op_mutation_does_not_advance_commit_revision() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("conversation.sqlite"),
        "cwp://com.example.runtime",
    )
    .expect("open store");
    let ConversationAdmissionOutcome::Started(turn) = store
        .admit(
            &graph,
            &ingress(
                "revision-no-op",
                person,
                None,
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![helper.into()],
                },
                "Initial message",
            ),
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .expect("admit conversation")
    else {
        panic!("expected new conversation")
    };
    let revision = |_: &ConversationAdmissionStore| cwpdb_store::test_support::timeline_persistence::commit_counter(&temporary.path().join("conversation.sqlite")).unwrap();
    let before = revision(&store);

    assert!(!store
        .invite_actor(&graph, &turn.record.conversation_ref, person, helper)
        .expect("reinviting a participant is a valid no-op"));

    assert_eq!(revision(&store), before);
}

#[test]
fn stale_open_writer_cannot_overwrite_a_newer_runtime_revision() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let path = temporary.path().join("conversation.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut initial = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime")
        .expect("open initial store");
    let ConversationAdmissionOutcome::Started(turn) = initial
        .admit(
            &graph,
            &ingress(
                "revision-concurrent-writers",
                person,
                None,
                ConversationIngressAddressing::Unaddressed,
                "Initial message",
            ),
            None,
        )
        .expect("admit conversation")
    else {
        panic!("expected new conversation")
    };
    let conversation_ref = turn.record.conversation_ref.clone();
    drop(initial);

    let mut first = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime")
        .expect("open first writer");
    let mut stale = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime")
        .expect("open stale writer");
    first
        .rename_conversation(&conversation_ref, person, "First writer")
        .expect("first writer commits");

    let error = stale
        .rename_conversation(&conversation_ref, person, "Stale writer")
        .expect_err("stale writer must fail");
    assert!(error.to_string().contains("another open writer"));
    assert_eq!(
        stale
            .conversation(&conversation_ref)
            .expect("stale state rolls back")
            .title
            .as_deref(),
        None
    );

    let reopened = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime")
        .expect("reopen current state");
    assert_eq!(
        reopened
            .conversation(&conversation_ref)
            .expect("retained conversation")
            .title
            .as_deref(),
        Some("First writer")
    );
}
