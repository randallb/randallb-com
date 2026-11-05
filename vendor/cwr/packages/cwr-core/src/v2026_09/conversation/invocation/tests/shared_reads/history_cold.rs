use super::*;
use crate::v2026_09::conversation::HistoryProjection;
#[test]
fn browse_selected_large_records_individually_without_shrinking_the_window() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
        unreachable!()
    };
    let template = prepared.conversation.messages[0].clone();
    let mut large = Vec::new();
    for n in 0..4 {
        let mut message = template.clone();
        message.message_ref = format!("cwp://com.example.fixture/messages/large-{n}");
        message.text = format!("needle-{n}");
        prepared.conversation.messages.push(message.clone());
        large.push(message);
    }
    store
        .commit(ConversationPreparationOutcome::New(prepared))
        .unwrap();
    for message in &mut large {
        message.text = format!("needle{}", "x".repeat(7 * 1024 * 1024));
        store
            .backend
            .lock()
            .unwrap()
            .commit_changes(cwpdb_store::conversation::Changes {
                records: vec![cwpdb_store::conversation::WriteRecord {
                    reference: message.message_ref.clone(),
                    kind: cwpdb_store::conversation::RecordKind::Message,
                    properties: serde_json::json!({"record":message}),
                    owner_ref: None,
                }],
                ..Default::default()
            })
            .unwrap();
    }
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(30)).unwrap();
    let window = ConversationRecordWindow {
        first: Some(5),
        ..Default::default()
    };
    let page = operation
        .conversation_browse_page(CONVERSATION_REF, window, 32)
        .unwrap();
    assert_eq!(page.total_count, 5);
    assert_eq!(page.content.len(), 5);
    assert!(!page.has_next_page);
    let content = operation
        .conversation_content(CONVERSATION_REF, window)
        .unwrap();
    assert_eq!(content.content.len(), 5);
    assert_eq!(content.total_count, 5);
    assert_eq!(content.content[4].message.text.len(), 7 * 1024 * 1024 + 6);
    let found = operation
        .conversation_history_search(CONVERSATION_REF, "needle", None, None, 5, 32)
        .unwrap();
    assert_eq!(found.content.len(), 4);
    assert!(!found.has_next_page);
    let archive = operation
        .conversation_history(CONVERSATION_REF, window)
        .unwrap();
    assert_eq!(archive.content.len(), 5);
    assert_eq!(archive.content[4].message.text.len(), 7 * 1024 * 1024 + 6);
    let control = cwpdb_store::conversation::ReadControl {
        cancellation: Default::default(),
        deadline: std::time::Instant::now() + Duration::from_secs(30),
        vm_steps: None,
    };
    let live = store.canonical_history_reader(CONVERSATION_REF, None, false, control);
    let page = live
        .conversation_browse_page(CONVERSATION_REF, window, 32)
        .unwrap();
    assert_eq!(page.content.len(), 5);
    assert_eq!(page.total_count, 5);
}

#[test]
fn retained_content_is_independent_of_a_newer_streaming_presentation() {
    for canonical in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stream.sqlite");
        let mut store = open(&path);
        let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
            unreachable!()
        };
        prepared.conversation.messages[0].text = "Retained answer".into();
        let message = prepared.conversation.messages[0].clone();
        store
            .commit(ConversationPreparationOutcome::New(prepared))
            .unwrap();
        if canonical {
            store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
            store
                .publish_root_message_completed(
                    ROOT_INVOCATION_REF,
                    ActionResponseStatus::COMPLETED,
                    json!({"text":"Done"}),
                    "Done".into(),
                    50,
                )
                .unwrap();
            store
                .configure_timeline_rollout(
                    crate::v2026_09::conversation::ConversationTimelineRollout::MigrateLegacy,
                )
                .unwrap();
            store.migrate_eligible_legacy_timelines().unwrap();
            assert!(store
                .state
                .conversations
                .iter()
                .find(|c| c.conversation_ref == CONVERSATION_REF)
                .unwrap()
                .timeline_connection_ref
                .is_some());
        }
        store.backend.lock().unwrap().commit_changes(cwpdb_store::conversation::Changes{records:vec![cwpdb_store::conversation::WriteRecord{reference:message.message_ref.clone(),kind:cwpdb_store::conversation::RecordKind::Message,properties:serde_json::json!({"streaming":{"conversation_ref":CONVERSATION_REF,"message":{"message_ref":message.message_ref,"author_actor_ref":message.author_actor_ref,"addressed_actor_refs":[],"text":"Live draft"}}}),owner_ref:None}],..Default::default()}).unwrap();
        assert!(store
            .backend
            .lock()
            .unwrap()
            .get(&message.message_ref)
            .unwrap()
            .unwrap()
            .properties["streaming"]
            .is_object());
        let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
        let op = reader.begin_operation(Duration::from_secs(5)).unwrap();
        let page = op
            .conversation_content(
                CONVERSATION_REF,
                ConversationRecordWindow {
                    first: Some(5),
                    ..Default::default()
                },
            )
            .unwrap();
        if canonical {
            assert!(page
                .content
                .iter()
                .all(|item| item.message.message_ref != message.message_ref));
        } else {
            assert_eq!(page.content.len(), 1);
            assert_eq!(page.content[0].message.text, "Retained answer");
        }
    }
}
