use super::*;

#[test]
fn browse_case_folding_preserves_text_key_and_value_offsets() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
        unreachable!()
    };
    let message = &mut prepared.conversation.messages[0];
    message.text = "pré Straße Σς ﬃ".into();
    message.action = Some(ConversationActionEvent::Result {
        activation: "@worker--lookup".into(),
        result: serde_json::json!({"groß": "Maße"}),
        outcome: Some(ConversationActionOutcome::Succeeded),
    });
    store
        .commit(ConversationPreparationOutcome::New(prepared))
        .unwrap();
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
    for (query, expected, offset) in [
        ("STRASSE", "Straße", 5),
        ("SSE", "ße", 9),
        ("σσ", "Σς", 13),
        ("FFI", "ﬃ", 18),
    ] {
        let page = operation
            .conversation_history_search(CONVERSATION_REF, query, None, None, 1, 64)
            .unwrap();
        let entry = &page.content[0];
        assert_eq!(entry.excerpt_offset_bytes, offset, "{query}");
        assert!(entry.text_excerpt.starts_with(expected), "{query}");
    }
    for (query, expected) in [("GROSS", "groß"), ("MASSE", "Maße")] {
        let page = operation
            .conversation_history_search(CONVERSATION_REF, query, None, None, 1, 64)
            .unwrap();
        assert!(page.content[0]
            .action_payload_json_excerpt
            .as_ref()
            .unwrap()
            .starts_with(expected));
    }
}

#[test]
fn browse_rejects_oversized_records_but_full_archive_remains_readable() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
        unreachable!()
    };
    let oversized_ref = "cwp://com.example.fixture/messages/oversized";
    let large = format!("private-payload-marker{}", "x".repeat(8 * 1024 * 1024));
    let mut oversized = prepared.conversation.messages[0].clone();
    oversized.message_ref = oversized_ref.into();
    oversized.text = large.clone();
    prepared.conversation.messages.push(oversized);
    store
        .commit(ConversationPreparationOutcome::New(prepared))
        .unwrap();
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
    let payload_bytes =
        cwpdb_store::test_support::invocation_reads::message_payload_bytes(&path, oversized_ref)
            .unwrap();
    let window = ConversationRecordWindow {
        last: Some(2),
        ..Default::default()
    };
    let errors = [
        (
            "conversation_browse_page",
            operation
                .conversation_browse_page(CONVERSATION_REF, window, 64)
                .unwrap_err(),
        ),
        (
            "conversation_history_around",
            operation
                .conversation_history_around(CONVERSATION_REF, oversized_ref, 0, 0, 64)
                .unwrap_err(),
        ),
        (
            "conversation_message_excerpt",
            operation
                .conversation_message_excerpt(CONVERSATION_REF, oversized_ref, 0, 0, 64)
                .unwrap_err(),
        ),
        (
            "conversation_history_search",
            operation
                .conversation_history_search(CONVERSATION_REF, "missing", None, None, 1, 64)
                .unwrap_err(),
        ),
    ];
    for (expected_operation, error) in errors {
        assert!(error.to_string().contains("8 MiB browse limit"), "{error}");
        let overage = crate::v2026_09::conversation::BrowseMessageOverage::in_error(error.as_ref())
            .expect("typed browse overage");
        assert_eq!(overage.conversation_ref, CONVERSATION_REF);
        assert_eq!(overage.message_ref, oversized_ref);
        assert_eq!(overage.operation, expected_operation);
        assert_eq!(overage.payload_bytes, payload_bytes as usize);
        assert_eq!(overage.limit_bytes, 8 * 1024 * 1024);
        assert!(!error.to_string().contains("private-payload-marker"));
    }
    let archive = operation
        .conversation_history(CONVERSATION_REF, window)
        .unwrap();
    assert_eq!(archive.content[1].message.text, large);
}
