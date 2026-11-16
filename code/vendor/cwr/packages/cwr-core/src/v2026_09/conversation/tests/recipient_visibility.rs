#[test]
fn unresolved_turns_make_each_reply_recipient_explicit() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let state_path = temporary.path().join("conversation.json");
    let mut store = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
        .expect("open store");
    let graph = fixture_graph_with_reviewer();
    let first = store
        .admit(
            &graph,
            &ingress(
                "event-first-pending",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                "@helper first",
            ),
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .expect("admit first turn");
    let ConversationAdmissionOutcome::Started(first) = first else {
        panic!("first admission must start a turn")
    };
    assert_eq!(
        first.record.reply_recipient_visibility,
        ConversationRecipientVisibility::Implicit
    );

    let second = store
        .admit(
            &graph,
            &ingress(
                "event-second-pending",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&first.record.conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "second",
            ),
            None,
        )
        .expect("admit second unresolved turn");
    let ConversationAdmissionOutcome::Started(second) = second else {
        panic!("second admission must start a turn")
    };
    assert_eq!(
        second.record.reply_recipient_visibility,
        ConversationRecipientVisibility::Explicit
    );
    assert_eq!(
        store
            .admission(
                &first.record.integration_ref,
                &first.record.provider_event_ref,
            )
            .unwrap()
            .reply_recipient_visibility,
        ConversationRecipientVisibility::Explicit
    );

    let reopened = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
        .expect("reopen store");
    assert!(reopened
        .admissions()
        .filter(|record| record.conversation_ref == first.record.conversation_ref)
        .all(|record| {
            record.reply_recipient_visibility == ConversationRecipientVisibility::Explicit
        }));
}
