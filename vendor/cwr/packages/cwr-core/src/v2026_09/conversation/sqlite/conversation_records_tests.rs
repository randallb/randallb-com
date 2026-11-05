use super::*;

#[test]
fn targeted_conversation_load_matches_full_load_with_transfer_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("targeted-conversation.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://example.test/runtime").unwrap();
    let root = "cwp://example.test/conversations/root";
    let other = "cwp://example.test/conversations/other";
    let first_message = "cwp://example.test/messages/first";
    let second_message = "cwp://example.test/messages/second";
    let first_activity = "cwp://example.test/activities/first";
    let transfer_activity = "cwp://example.test/activities/association";
    let last_activity = "cwp://example.test/activities/last";

    let retained: RetainedConversation = serde_json::from_value(serde_json::json!({
        "owner_context_ref": "cwp://example.test/owners/one",
        "conversation_ref": root,
        "title": "A retained conversation",
        "archived": true,
        "sidebar_eligible": false,
        "participant_actor_refs": ["cwp://example.test/actors/person"],
        "messages": [
            {
                "message_ref": first_message,
                "author_actor_ref": "cwp://example.test/actors/person",
                "addressed_actor_refs": [],
                "text": "first\n\u{e200}visualize\u{e202}{\"path\":\"/workspace/chart.html\"}\u{e201}",
                "visualizations": [{"marker_index": 0, "title": "Chart", "error": "unavailable"}]
            },
            {
                "message_ref": second_message,
                "author_actor_ref": "cwp://example.test/actors/person",
                "addressed_actor_refs": [],
                "text": "second"
            }
        ],
        "activity": [
            {
                "activity_ref": first_activity,
                "sequence": 1,
                "event": {"kind": "message_recorded", "message_ref": first_message}
            },
            {
                "activity_ref": last_activity,
                "sequence": 3,
                "event": {"kind": "message_recorded", "message_ref": second_message}
            },
            {
                "activity_ref": transfer_activity,
                "sequence": 2,
                "event": {
                    "kind": "message_work_association",
                    "message_ref": first_message,
                    "activity_ref": first_activity
                }
            }
        ]
    }))
    .unwrap();
    let unrelated: RetainedConversation = serde_json::from_value(serde_json::json!({
        "conversation_ref": other,
        "participant_actor_refs": [],
        "messages": [{
            "message_ref": "cwp://example.test/messages/other",
            "author_actor_ref": "cwp://example.test/actors/person",
            "addressed_actor_refs": [],
            "text": "unrelated"
        }]
    }))
    .unwrap();
    let mut state = store.state.clone();
    state.conversations = vec![retained, unrelated].into();
    store.replace_state(state).unwrap();

    let mut backend = store.backend.lock().unwrap();
    let transfer_rows = backend
        .pages(cwpdb_store::conversation::Selection::Records {
            kind: cwpdb_store::conversation::RecordKind::ConversationTransferMetadata,
        })
        .unwrap()
        .records
        .len();
    assert_eq!(transfer_rows, 1);
    // Expected full protocol data uses canonical activity sequence, including
    // transfer metadata deliberately inserted out of order in the fixture.
    let all = store
        .state
        .conversations
        .iter()
        .cloned()
        .map(|mut conversation| {
            conversation
                .activity
                .sort_by_key(|activity| activity.sequence);
            conversation
        })
        .collect::<Vec<_>>();
    let bootstrap = backend
        .pages(cwpdb_store::conversation::Selection::Bootstrap)
        .unwrap();
    let headers = super::super::backend::fixture_conversation_headers(
        &bootstrap,
        "cwp://example.test/runtime",
    )
    .unwrap();
    assert_eq!(headers.len(), all.len());
    assert_eq!(headers[0].conversation_ref, root);
    assert!(headers
        .iter()
        .all(|item| item.messages.is_empty() && item.activity.is_empty()));
    for reference in [root, other] {
        let targeted = backend.conversation(reference).unwrap();
        assert_eq!(
            &targeted,
            all.iter()
                .find(|row| row.conversation_ref == reference)
                .unwrap()
        );
    }
    let root = backend.conversation(root).unwrap();
    assert_eq!(
        root.activity
            .iter()
            .map(|item| item.activity_ref.as_str())
            .collect::<Vec<_>>(),
        [first_activity, transfer_activity, last_activity]
    );
    assert_eq!(
        root.messages[0].visualizations[0].title.as_deref(),
        Some("Chart")
    );
    assert!(backend
        .get("cwp://example.test/conversations/missing")
        .unwrap()
        .is_none());
}
