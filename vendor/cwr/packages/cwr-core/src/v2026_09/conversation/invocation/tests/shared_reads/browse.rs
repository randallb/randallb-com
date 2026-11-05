use super::*;

#[test]
fn conversation_browse_anchors_searches_and_returns_bounded_continuations() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
        unreachable!("root fixture is always new")
    };
    for (message_ref, text) in [
        ("reply-one", "Earlier result"),
        ("reply-two", "Later result"),
    ] {
        prepared.conversation.messages.push(ConversationMessage {
            file_refs: Vec::new(),
            progress: None,
            message_ref: format!("cwp://com.example.fixture/messages/{message_ref}"),
            author_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
            addressed_actor_refs: Vec::new(),
            addressing_mode: Default::default(),
            text: text.into(),
            visualizations: Vec::new(),
            reply_to_message_ref: Some(SOURCE_MESSAGE_REF.into()),
            action: Some(ConversationActionEvent::Result {
                activation: "@worker--lookup".into(),
                result: serde_json::json!({"found": true}),
                outcome: Some(ConversationActionOutcome::Succeeded),
            }),
        });
    }
    store
        .commit(ConversationPreparationOutcome::New(prepared))
        .unwrap();
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();

    assert!(operation
        .conversation_browse_page(
            CONVERSATION_REF,
            ConversationRecordWindow {
                last: Some(1),
                ..Default::default()
            },
            0,
        )
        .is_err());
    assert!(operation
        .conversation_history_around(CONVERSATION_REF, SOURCE_MESSAGE_REF, 0, 0, 0)
        .is_err());
    assert!(operation
        .conversation_history_search(CONVERSATION_REF, "help", None, None, 1, 0)
        .is_err());

    let recent = operation
        .conversation_browse_page(
            CONVERSATION_REF,
            ConversationRecordWindow {
                first: Some(1),
                ..Default::default()
            },
            4,
        )
        .unwrap();
    assert_eq!(recent.content[0].message_ref, SOURCE_MESSAGE_REF);
    assert_eq!(recent.content[0].text_excerpt, "Plea");
    assert_eq!(recent.content[0].next_offset_bytes, Some(4));

    let continuation = operation
        .conversation_message_excerpt(CONVERSATION_REF, SOURCE_MESSAGE_REF, 4, 0, 4)
        .unwrap();
    assert_eq!(continuation.text_excerpt, "se h");
    assert_eq!(continuation.excerpt_offset_bytes, 4);

    let around = operation
        .conversation_history_around(CONVERSATION_REF, SOURCE_MESSAGE_REF, 5, 5, 32)
        .unwrap();
    assert_eq!(around.content.len(), 3);
    assert_eq!(around.content[0].message_ref, SOURCE_MESSAGE_REF);

    let found = operation
        .conversation_history_search(CONVERSATION_REF, "help", None, None, 5, 32)
        .unwrap();
    assert_eq!(found.content[0].message_ref, SOURCE_MESSAGE_REF);
    let result_search = operation
        .conversation_history_search(CONVERSATION_REF, "result", None, None, 1, 32)
        .unwrap();
    assert!(result_search.has_next_page);
    let next_result_search = operation
        .conversation_history_search(
            CONVERSATION_REF,
            "result",
            result_search.next_after_position,
            None,
            1,
            32,
        )
        .unwrap();
    assert_eq!(next_result_search.content[0].text_excerpt, "result");

    let links = operation
        .conversation_message_links(CONVERSATION_REF, SOURCE_MESSAGE_REF, None, None, 1)
        .unwrap();
    assert_eq!(links.reply_to_message_ref, None);
    assert_eq!(links.reply_message_refs.len(), 1);
    assert!(links.has_next_replies);
    let next_links = operation
        .conversation_message_links(
            CONVERSATION_REF,
            SOURCE_MESSAGE_REF,
            links.next_after_message_ref.as_deref(),
            None,
            1,
        )
        .unwrap();
    assert_eq!(next_links.reply_message_refs.len(), 1);
    assert!(!next_links.has_next_replies);
    let reply = operation
        .conversation_message_excerpt(
            CONVERSATION_REF,
            &next_links.reply_message_refs[0],
            0,
            0,
            32,
        )
        .unwrap();
    assert_eq!(reply.text_excerpt, "Later result");
    assert_eq!(
        reply.reply_to_message_ref.as_deref(),
        Some(SOURCE_MESSAGE_REF)
    );
    assert_eq!(reply.action_kind.as_deref(), Some("result"));
    assert_eq!(reply.action_activation.as_deref(), Some("@worker--lookup"));
    assert_eq!(reply.action_payload_field.as_deref(), Some("result"));
    assert_eq!(
        reply.action_payload_json_excerpt.as_deref(),
        Some("{\"found\":true}")
    );
    assert_eq!(
        reply.action_outcome,
        Some(ConversationActionOutcome::Succeeded)
    );

    let payload_search = operation
        .conversation_history_search(CONVERSATION_REF, "true", None, None, 5, 32)
        .unwrap();
    let payload_hit = payload_search
        .content
        .iter()
        .find(|entry| entry.message_ref == next_links.reply_message_refs[0])
        .unwrap();
    assert_eq!(payload_hit.action_payload_field.as_deref(), Some("result"));
    assert!(payload_hit
        .action_payload_json_excerpt
        .as_deref()
        .unwrap()
        .contains("true"));
    let payload_continuation = operation
        .conversation_message_excerpt(
            CONVERSATION_REF,
            &payload_hit.message_ref,
            0,
            payload_hit.action_payload_offset_bytes.unwrap(),
            5,
        )
        .unwrap();
    assert!(payload_continuation
        .action_payload_json_excerpt
        .as_deref()
        .unwrap()
        .contains("true"));
}

#[test]
fn conversation_browse_caps_the_serialized_page_size() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
        unreachable!("root fixture is always new")
    };
    for index in 0..20 {
        prepared.conversation.messages.push(ConversationMessage {
            file_refs: Vec::new(),
            progress: None,
            message_ref: format!("cwp://com.example.fixture/messages/large-{index}"),
            author_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
            addressed_actor_refs: Vec::new(),
            addressing_mode: Default::default(),
            text: "x".repeat(4_096),
            visualizations: Vec::new(),
            reply_to_message_ref: None,
            action: None,
        });
    }
    store
        .commit(ConversationPreparationOutcome::New(prepared))
        .unwrap();
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();

    let page = operation
        .conversation_browse_page(
            CONVERSATION_REF,
            ConversationRecordWindow {
                first: Some(25),
                ..Default::default()
            },
            2_048,
        )
        .unwrap();
    let encoded = serde_json::to_vec(&page).unwrap();
    assert!(encoded.len() <= MAX_CONVERSATION_BROWSE_RESPONSE_BYTES);
    assert!(page.has_next_page);
    assert!(!page.content.is_empty());

    let last_page = operation
        .conversation_browse_page(
            CONVERSATION_REF,
            ConversationRecordWindow {
                last: Some(20),
                ..Default::default()
            },
            2_048,
        )
        .unwrap();
    assert_eq!(
        last_page.content.last().unwrap().message_ref,
        "cwp://com.example.fixture/messages/large-19"
    );

    let around = operation
        .conversation_history_around(
            CONVERSATION_REF,
            "cwp://com.example.fixture/messages/large-10",
            10,
            10,
            2_048,
        )
        .unwrap();
    assert!(around
        .content
        .iter()
        .any(|entry| { entry.message_ref == "cwp://com.example.fixture/messages/large-10" }));
}

#[test]
fn conversation_browse_search_offsets_and_excerpt_limits_are_utf8_safe() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
        unreachable!("root fixture is always new")
    };
    let unicode_ref = "cwp://com.example.fixture/messages/unicode";
    prepared.conversation.messages.push(ConversationMessage {
        file_refs: Vec::new(),
        progress: None,
        message_ref: unicode_ref.into(),
        author_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
        addressed_actor_refs: Vec::new(),
        addressing_mode: Default::default(),
        text: format!("{}needle", "é".repeat(100)),
        visualizations: Vec::new(),
        reply_to_message_ref: None,
        action: None,
    });
    for index in 0..24 {
        prepared.conversation.messages.push(ConversationMessage {
            file_refs: Vec::new(),
            progress: None,
            message_ref: format!("cwp://com.example.fixture/messages/unicode-page-{index}"),
            author_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
            addressed_actor_refs: Vec::new(),
            addressing_mode: Default::default(),
            text: "é".repeat(2_048),
            visualizations: Vec::new(),
            reply_to_message_ref: None,
            action: None,
        });
    }
    let escaped_payload_ref = "cwp://com.example.fixture/messages/escaped-payload";
    prepared.conversation.messages.push(ConversationMessage {
        file_refs: Vec::new(),
        progress: None,
        message_ref: escaped_payload_ref.into(),
        author_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
        addressed_actor_refs: Vec::new(),
        addressing_mode: Default::default(),
        text: "Structured result follows".into(),
        visualizations: Vec::new(),
        reply_to_message_ref: None,
        action: Some(ConversationActionEvent::Result {
            activation: "@worker--lookup".into(),
            result: serde_json::Value::String(format!("{}C:\\target\\needle", "x".repeat(3_000))),
            outcome: Some(ConversationActionOutcome::Succeeded),
        }),
    });
    let nested_payload_ref = "cwp://com.example.fixture/messages/nested-payload";
    prepared.conversation.messages.push(ConversationMessage {
        file_refs: Vec::new(),
        progress: None,
        message_ref: nested_payload_ref.into(),
        author_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
        addressed_actor_refs: Vec::new(),
        addressing_mode: Default::default(),
        text: "Done".into(),
        visualizations: Vec::new(),
        reply_to_message_ref: None,
        action: Some(ConversationActionEvent::Result {
            activation: "@worker--lookup".into(),
            result: serde_json::json!({
                "stdout": format!("{}C:\\target\\needle", "y".repeat(3_000))
            }),
            outcome: Some(ConversationActionOutcome::Succeeded),
        }),
    });
    let unicode_case_ref = "cwp://com.example.fixture/messages/unicode-case";
    prepared.conversation.messages.push(ConversationMessage {
        file_refs: Vec::new(),
        progress: None,
        message_ref: unicode_case_ref.into(),
        author_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
        addressed_actor_refs: Vec::new(),
        addressing_mode: Default::default(),
        text: "Éclair and Kelvin".into(),
        visualizations: Vec::new(),
        reply_to_message_ref: None,
        action: Some(ConversationActionEvent::Result {
            activation: "@worker--lookup".into(),
            result: serde_json::json!({"Étiquette": "Éclair"}),
            outcome: Some(ConversationActionOutcome::Succeeded),
        }),
    });
    store
        .commit(ConversationPreparationOutcome::New(prepared))
        .unwrap();
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();

    let search = operation
        .conversation_history_search(CONVERSATION_REF, "needle", None, None, 5, 16)
        .unwrap();
    let hit = search
        .content
        .iter()
        .find(|entry| entry.message_ref == unicode_ref)
        .unwrap();
    assert_eq!(hit.excerpt_offset_bytes, 200);
    assert!(hit.text_excerpt.contains("needle"));

    assert!(!operation
        .conversation_history_search(CONVERSATION_REF, &"é".repeat(200), None, None, 5, 16)
        .unwrap()
        .content
        .is_empty());
    assert!(operation
        .conversation_history_search(CONVERSATION_REF, &"x".repeat(257), None, None, 5, 16)
        .is_err());

    let too_small = operation.conversation_message_excerpt(CONVERSATION_REF, unicode_ref, 0, 0, 1);
    assert!(too_small.is_err());

    let unicode_page = operation
        .conversation_browse_page(
            CONVERSATION_REF,
            ConversationRecordWindow {
                last: Some(25),
                ..Default::default()
            },
            2_048,
        )
        .unwrap();
    assert!(!unicode_page.content.is_empty());
    assert!(
        serde_json::to_vec(&unicode_page).unwrap().len() <= MAX_CONVERSATION_BROWSE_RESPONSE_BYTES
    );
    for entry in &unicode_page.content {
        if entry.text_bytes > entry.excerpt_offset_bytes {
            assert!(!entry.text_excerpt.is_empty());
            assert!(
                entry.text_excerpt.len() + entry.excerpt_offset_bytes == entry.text_bytes
                    || entry
                        .next_offset_bytes
                        .is_some_and(|next| next > entry.excerpt_offset_bytes)
            );
        }
    }

    let escaped_search = operation
        .conversation_history_search(CONVERSATION_REF, "C:\\target", None, None, 25, 64)
        .unwrap();
    let escaped_hit = escaped_search
        .content
        .iter()
        .find(|entry| entry.message_ref == escaped_payload_ref)
        .unwrap();
    assert_eq!(escaped_hit.action_payload_field.as_deref(), Some("result"));
    assert!(escaped_hit
        .action_payload_json_excerpt
        .as_deref()
        .unwrap()
        .contains(r"C:\\target"));
    let nested_hit = escaped_search
        .content
        .iter()
        .find(|entry| entry.message_ref == nested_payload_ref)
        .unwrap();
    assert_eq!(nested_hit.action_payload_field.as_deref(), Some("result"));
    assert!(nested_hit
        .action_payload_json_excerpt
        .as_deref()
        .unwrap()
        .contains(r"C:\\target"));
    let wrapper_search = operation
        .conversation_history_search(CONVERSATION_REF, "result", None, None, 25, 64)
        .unwrap();
    assert!(!wrapper_search
        .content
        .iter()
        .any(|entry| entry.message_ref == nested_payload_ref));
    for query in ["éclair", "kelvin", "étiquette"] {
        let matches = operation
            .conversation_history_search(CONVERSATION_REF, query, None, None, 25, 64)
            .unwrap();
        assert!(
            matches
                .content
                .iter()
                .any(|entry| entry.message_ref == unicode_case_ref),
            "missing Unicode case match for {query}"
        );
    }
}

#[test]
fn conversation_browse_rejects_oversized_excerpt_metadata() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
        unreachable!("root fixture is always new")
    };
    let large_metadata_ref = "cwp://com.example.fixture/messages/large-metadata";
    prepared.conversation.messages.push(ConversationMessage {
        file_refs: Vec::new(),
        progress: None,
        message_ref: large_metadata_ref.into(),
        author_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
        addressed_actor_refs: vec!["cwp://com.example.fixture/actors/worker".into(); 5_000],
        addressing_mode: Default::default(),
        text: "small".into(),
        visualizations: Vec::new(),
        reply_to_message_ref: None,
        action: None,
    });
    store
        .commit(ConversationPreparationOutcome::New(prepared))
        .unwrap();
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
    assert!(operation
        .conversation_message_excerpt(CONVERSATION_REF, large_metadata_ref, 0, 0, 16)
        .is_err());
}

#[test]
fn conversation_browse_minimum_excerpt_advances_over_four_byte_characters() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
        unreachable!()
    };
    prepared.conversation.messages[0].text = "🦀é".into();
    store
        .commit(ConversationPreparationOutcome::New(prepared))
        .unwrap();
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
    for bytes in 1..=4 {
        let recent = operation.conversation_browse_page(
            CONVERSATION_REF,
            ConversationRecordWindow {
                last: Some(1),
                ..Default::default()
            },
            bytes,
        );
        let around = operation.conversation_history_around(
            CONVERSATION_REF,
            SOURCE_MESSAGE_REF,
            0,
            0,
            bytes,
        );
        let search =
            operation.conversation_history_search(CONVERSATION_REF, "🦀", None, None, 1, bytes);
        let excerpt = operation.conversation_message_excerpt(
            CONVERSATION_REF,
            SOURCE_MESSAGE_REF,
            0,
            0,
            bytes,
        );
        if bytes < 4 {
            assert!(recent.is_err() && around.is_err() && search.is_err() && excerpt.is_err());
        } else {
            for entry in [
                recent.unwrap().content.remove(0),
                around.unwrap().content.remove(0),
                search.unwrap().content.remove(0),
                excerpt.unwrap(),
            ] {
                assert_eq!(entry.text_excerpt, "🦀");
                assert_eq!(entry.next_offset_bytes, Some(4));
            }
        }
    }
}

include!("browse_visualizations.rs");
