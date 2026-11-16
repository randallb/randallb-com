#[test]
fn conversation_titles_are_normalized_validated_and_persisted() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let state_path = temporary.path().join("conversation.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
        .expect("open store");
    let admitted = store
        .admit(
            &graph,
            &ingress(
                "title-source",
                person,
                None,
                ConversationIngressAddressing::Unaddressed,
                "Initial message",
            ),
            None,
        )
        .expect("admit conversation");
    let ConversationAdmissionOutcome::Started(turn) = admitted else {
        panic!("expected new conversation")
    };
    let conversation_ref = turn.conversation.conversation_ref;

    let renamed = store
        .rename_conversation(&conversation_ref, person, "  Apple local adapter design  ")
        .expect("rename conversation");
    assert_eq!(renamed.title.as_deref(), Some("Apple local adapter design"));

    let renamed = store
        .rename_conversation(&conversation_ref, person, "\u{feff}Shared plan\u{feff}")
        .expect("trim BOM padding");
    assert_eq!(renamed.title.as_deref(), Some("Shared plan"));

    for invalid_title in [
        "   ",
        "two\nlines",
        "control\u{7}",
        "line\u{2028}separator",
        "paragraph\u{2029}separator",
    ] {
        assert!(store
            .rename_conversation(&conversation_ref, person, invalid_title)
            .is_err());
    }
    assert!(store
        .rename_conversation(
            &conversation_ref,
            "cwp://com.example.fixture/actors/stranger",
            "Unauthorized"
        )
        .is_err());
    assert_eq!(
        store
            .conversation(&conversation_ref)
            .unwrap()
            .title
            .as_deref(),
        Some("Shared plan")
    );

    drop(store);
    let reopened = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
        .expect("reopen store");
    assert_eq!(
        reopened
            .conversation(&conversation_ref)
            .unwrap()
            .title
            .as_deref(),
        Some("Shared plan")
    );
}

#[test]
fn conversation_archive_state_is_shared_authorized_and_persisted() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let state_path = temporary.path().join("conversation.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
        .expect("open store");
    let ConversationAdmissionOutcome::Started(first) = store
        .admit(
            &graph,
            &ingress(
                "archive-source",
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
    let conversation_ref = first.record.conversation_ref;

    assert!(store
        .set_conversation_archived(
            &conversation_ref,
            "cwp://com.example.fixture/actors/stranger",
            true
        )
        .is_err());
    assert!(
        store
            .set_conversation_archived(&conversation_ref, person, true)
            .expect("archive conversation")
            .archived
    );

    let prepared = store
        .prepare(
            &graph,
            &ingress(
                "archive-continued",
                person,
                Some(&conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "Continue while archived",
            ),
            None,
        )
        .expect("prepare continued message");
    store.commit(prepared).expect("commit continued message");
    assert!(store.conversation(&conversation_ref).unwrap().archived);

    drop(store);
    let mut reopened = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
        .expect("reopen store");
    assert!(reopened.conversation(&conversation_ref).unwrap().archived);
    assert!(
        !reopened
            .set_conversation_archived(&conversation_ref, person, false)
            .expect("restore conversation")
            .archived
    );
}

#[test]
fn prepared_ingress_preserves_a_newer_conversation_title() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("conversation.sqlite"),
        "cwp://com.example.runtime",
    )
    .expect("open store");
    let ConversationAdmissionOutcome::Started(first) = store
        .admit(
            &graph,
            &ingress(
                "prepared-title-first",
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
    let conversation_ref = first.record.conversation_ref.clone();
    let prepared = store
        .prepare(
            &graph,
            &ingress(
                "prepared-title-second",
                person,
                Some(&conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "Prepared before rename",
            ),
            None,
        )
        .expect("prepare continued message");

    store
        .rename_conversation(&conversation_ref, person, "Newer title")
        .expect("rename conversation");
    store.commit(prepared).expect("commit prepared message");

    assert_eq!(
        store
            .conversation(&conversation_ref)
            .unwrap()
            .title
            .as_deref(),
        Some("Newer title")
    );
}

#[test]
fn conversation_revision_advances_for_activity_and_earlier_message_changes() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let state_path = temporary.path().join("conversation.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
        .expect("open store");
    let ConversationAdmissionOutcome::Started(first) = store
        .admit(
            &graph,
            &ingress(
                "revision-first",
                person,
                None,
                ConversationIngressAddressing::Unaddressed,
                "First message",
            ),
            None,
        )
        .expect("admit first message")
    else {
        panic!("expected new conversation")
    };
    let conversation_ref = first.record.conversation_ref;
    store
        .admit(
            &graph,
            &ingress(
                "revision-second",
                person,
                Some(&conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "Second message",
            ),
            None,
        )
        .expect("admit second message");

    let revision = |store: &ConversationAdmissionStore| {
        let (path,runtime)=store.read_store_identity();
        cwpdb_store::test_support::conversation_observations::header_revision(&path,&runtime,&conversation_ref).unwrap()
    };
    let initial_revision = revision(&store);

    let mut activity_state = store.state.clone();
    activity_state
        .conversations
        .iter_mut()
        .find(|conversation| conversation.conversation_ref == conversation_ref)
        .unwrap()
        .activity
        .push(ConversationActivity {
            activity_ref: "cwp://com.example.runtime/activities/revision-test".into(),
            sequence: 99,
            occurred_at_unix_ms: 1,
            event: ConversationActivityEvent::MessageRecorded {
                message_ref: "revision-activity-message".into(),
            },
        });
    store.replace_state(activity_state).unwrap();
    let activity_revision = revision(&store);
    assert!(activity_revision > initial_revision);

    let mut message_state = store.state.clone();
    message_state
        .conversations
        .iter_mut()
        .find(|conversation| conversation.conversation_ref == conversation_ref)
        .unwrap()
        .messages[0]
        .text = "Edited first message".into();
    store.replace_state(message_state).unwrap();
    assert!(revision(&store) > activity_revision);
}
