#[test]
fn conversation_browse_projects_visualizations_before_search_and_excerpt() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let mut store = open(&path);
    let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
        unreachable!("root fixture is always new")
    };
    let message_ref = "cwp://com.example.fixture/messages/chart";
    let private_path = "/Users/example/private/chart.html";
    let marker = format!(
        "\u{e200}visualize\u{e202}{{\"path\":\"{private_path}\",\"title\":\"Saved chart\"}}\u{e201}"
    );
    prepared.conversation.messages.push(ConversationMessage {
        file_refs: Vec::new(),
        progress: None,
        message_ref: message_ref.into(),
        author_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
        addressed_actor_refs: Vec::new(),
        addressing_mode: Default::default(),
        text: format!("Summary\n\n{marker}"),
        visualizations: vec![crate::v2026_09::conversation::ConversationVisualization {
            marker_index: 0,
            title: Some("Saved chart".into()),
            mode: None,
            artifact_ref: None,
            error: Some("unavailable".into()),
        }],
        reply_to_message_ref: None,
        action: None,
    });
    store
        .commit(ConversationPreparationOutcome::New(prepared))
        .unwrap();
    let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();

    let exact = operation
        .conversation_history(
            CONVERSATION_REF,
            ConversationRecordWindow {
                last: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(exact.content[0].message.text.contains(private_path));
    let page = operation
        .conversation_browse_page(
            CONVERSATION_REF,
            ConversationRecordWindow {
                last: Some(1),
                ..Default::default()
            },
            256,
        )
        .unwrap();
    assert_eq!(page.content[0].text_excerpt, "Summary\n\nSaved chart");
    let excerpt = operation
        .conversation_message_excerpt(CONVERSATION_REF, message_ref, 0, 0, 256)
        .unwrap();
    assert_eq!(excerpt.text_excerpt, "Summary\n\nSaved chart");
    let around = operation
        .conversation_history_around(CONVERSATION_REF, message_ref, 1, 0, 256)
        .unwrap();
    assert_eq!(
        around.content.last().unwrap().text_excerpt,
        excerpt.text_excerpt
    );
    assert!(operation
        .conversation_history_search(CONVERSATION_REF, private_path, None, None, 5, 256)
        .unwrap()
        .content
        .is_empty());
    assert_eq!(
        operation
            .conversation_history_search(CONVERSATION_REF, "Saved chart", None, None, 5, 256)
            .unwrap()
            .content[0]
            .message_ref,
        message_ref
    );
}

