#[test]
fn prepared_continuations_cannot_reassign_or_clear_the_owner() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("prepared.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let mut input = ingress(
        "prepared-root",
        person,
        None,
        ConversationIngressAddressing::Unaddressed,
        "Owned",
    );
    input.owner_context_ref = Some("pack".into());
    let ConversationAdmissionOutcome::Started(started) = store.admit(&graph, &input, None).unwrap()
    else {
        panic!("new conversation")
    };
    let reference = started.conversation.conversation_ref;
    let before = store.conversation(&reference).unwrap().clone();
    for owner in [None, Some("other-pack".to_owned())] {
        let next = ingress(
            "prepared-next",
            person,
            Some(&reference),
            ConversationIngressAddressing::Unaddressed,
            "Next",
        );
        let ConversationPreparationOutcome::New(mut prepared) =
            store.prepare(&graph, &next, None).unwrap()
        else {
            panic!("new turn")
        };
        prepared.conversation.owner_context_ref = owner;
        assert!(store
            .commit(ConversationPreparationOutcome::New(prepared))
            .is_err());
        assert_eq!(store.conversation(&reference).unwrap(), &before);
        assert!(store
            .admission(&next.integration_ref, &next.provider_event_ref)
            .is_none());
    }
    drop(store);
    let store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    assert_eq!(store.conversation(&reference).unwrap(), &before);
}

#[test]
fn legacy_create_replay_requires_the_retained_owner() {
    for kind in [
        ConversationIngressConversation::Create,
        ConversationIngressConversation::CreateForAction,
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let graph = fixture_graph_with_action();
        let mut store = ConversationAdmissionStore::open(
            temporary.path().join("legacy-replay.sqlite"),
            "cwp://com.example.runtime",
        )
        .unwrap();
        let mut input = ingress(
            "legacy-owned-retry",
            crate::v2026_09::test_fixture::actor_ref("person"),
            None,
            ConversationIngressAddressing::Action {
                actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
                activation: "@helper--draft--brief".into(),
                action_name: "draft--brief".into(),
                input: json!({}),
            },
            "@helper--draft--brief",
        );
        input.conversation = kind;
        let ConversationAdmissionOutcome::Started(started) = store
            .admit(
                &graph,
                &input,
                Some(crate::v2026_09::test_fixture::binding_ref("helper")),
            )
            .unwrap()
        else {
            panic!("new conversation")
        };
        store.state.admissions[0].ingress_fingerprint = None;
        assert!(matches!(
            store
                .admit(
                    &graph,
                    &input,
                    Some(crate::v2026_09::test_fixture::binding_ref("helper"))
                )
                .unwrap(),
            ConversationAdmissionOutcome::Existing(_)
        ));
        input.owner_context_ref = Some("added-owner".into());
        assert!(store
            .admit(
                &graph,
                &input,
                Some(crate::v2026_09::test_fixture::binding_ref("helper"))
            )
            .unwrap_err()
            .to_string()
            .contains("reused with different input"));
        assert_eq!(
            store
                .conversation(&started.conversation.conversation_ref)
                .unwrap()
                .owner_context_ref,
            None
        );
    }
}
