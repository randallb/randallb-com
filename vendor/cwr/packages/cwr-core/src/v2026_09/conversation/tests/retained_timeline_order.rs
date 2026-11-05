#[test]
fn canonical_message_reads_follow_first_flat_timeline_placement() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical-message-order.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let first = working_turn(&mut store, None, "canonical-order-first");
    let second = working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "canonical-order-second",
    );
    let conversation_ref = first.record.conversation_ref.clone();
    let first_ref = first.record.source_message_ref.clone();
    let second_ref = second.record.source_message_ref.clone();
    let timeline_ref = format!("{conversation_ref}/timeline");
    store
        .create_retained_timeline(&conversation_ref, &timeline_ref)
        .unwrap();
    for (name, message_ref) in [
        ("second", second_ref.as_str()),
        ("first", first_ref.as_str()),
        ("second-again", second_ref.as_str()),
    ] {
        store
            .publish_timeline_message(
                &conversation_ref,
                &format!("{conversation_ref}/publications/{name}"),
                message_ref,
                ConversationTimelineDisposition::Root,
            )
            .unwrap();
    }

    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    let history = operation
        .conversation_history(
            &conversation_ref,
            ConversationRecordWindow {
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(history.total_count, 2);
    assert_eq!(
        history
            .content
            .iter()
            .map(|entry| (entry.key.position, entry.message.message_ref.as_str()))
            .collect::<Vec<_>>(),
        [(0, second_ref.as_str()), (1, first_ref.as_str())]
    );

    let cursor = history.content[0].key.clone();
    let next = operation
        .conversation_content(
            &conversation_ref,
            ConversationRecordWindow {
                after: Some(&cursor),
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(next.content.len(), 1);
    assert_eq!(next.content[0].message.message_ref, first_ref);

    let search = operation
        .conversation_history_search(
            &conversation_ref,
            "canonical-order",
            None,
            None,
            1,
            256,
        )
        .unwrap();
    assert_eq!(search.content[0].message_ref, second_ref);
    let search_next = operation
        .conversation_history_search(
            &conversation_ref,
            "canonical-order",
            search.next_after_position,
            Some(&search.revision),
            1,
            256,
        )
        .unwrap();
    assert_eq!(search_next.content[0].message_ref, first_ref);

    let around = operation
        .conversation_history_around(&conversation_ref, &first_ref, 1, 0, 256)
        .unwrap();
    assert_eq!(
        around
            .content
            .iter()
            .map(|entry| entry.message_ref.as_str())
            .collect::<Vec<_>>(),
        [second_ref.as_str(), first_ref.as_str()]
    );
}
