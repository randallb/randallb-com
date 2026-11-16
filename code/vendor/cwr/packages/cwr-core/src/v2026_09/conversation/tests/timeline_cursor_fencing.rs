#[test]
fn canonical_message_and_search_cursors_are_fenced_by_visibility_revision() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical-message-cursors.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let exchange = store
        .record_host_exchange(
            None,
            crate::v2026_09::test_fixture::actor_ref("person"),
            crate::v2026_09::test_fixture::actor_ref("helper"),
            "cursor match request".into(),
            "cursor match response".into(),
        )
        .unwrap();
    let conversation_ref = exchange.conversation.conversation_ref;
    let root_connection_ref = format!("{conversation_ref}/timeline");
    store
        .create_retained_timeline(&conversation_ref, &root_connection_ref)
        .unwrap();
    store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/response"),
            &exchange.response_message_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();

    let (content_cursor, search_position, old_revision) = {
        let mut reader = store.read_store().unwrap();
        let operation = reader
            .begin_operation(std::time::Duration::from_secs(2))
            .unwrap();
        let content = operation
            .conversation_content(
                &conversation_ref,
                ConversationRecordWindow {
                    first: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
        let search = operation
            .conversation_history_search(
                &conversation_ref,
                "cursor match",
                None,
                None,
                1,
                256,
            )
            .unwrap();
        (
            content.content[0].key.clone(),
            search.next_after_position.unwrap(),
            search.revision,
        )
    };
    let mut missing_revision = content_cursor.clone();
    missing_revision.revision = None;
    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(operation
        .conversation_content(
            &conversation_ref,
            ConversationRecordWindow {
                after: Some(&missing_revision),
                first: Some(1),
                ..Default::default()
            },
        )
        .is_err());
    assert!(operation
        .conversation_history_search(
            &conversation_ref,
            "cursor match",
            Some(search_position),
            None,
            1,
            256,
        )
        .is_err());
    drop(operation);
    drop(reader);

    store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/request"),
            &exchange.request_message_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    let content_error = operation
        .conversation_content(
            &conversation_ref,
            ConversationRecordWindow {
                after: Some(&content_cursor),
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(
        content_error.to_string().contains("earlier revision"),
        "{content_error}"
    );
    let search_error = operation
        .conversation_history_search(
            &conversation_ref,
            "cursor match",
            Some(search_position),
            Some(&old_revision),
            1,
            256,
        )
        .unwrap_err();
    assert!(
        search_error.to_string().contains("earlier canonical timeline"),
        "{search_error}"
    );
}

#[test]
fn canonical_completion_cursors_are_fenced_by_visibility_revision() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical-completion-cursors.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let first = settled_legacy_turn(&mut store, None, "cursor-completion-first");
    let conversation_ref = first.record.conversation_ref.clone();
    let second = settled_legacy_turn(
        &mut store,
        Some(&conversation_ref),
        "cursor-completion-second",
    );
    let first_invocation_ref = first.record.invocation_ref.unwrap();
    let second_invocation_ref = second.record.invocation_ref.unwrap();
    let root_connection_ref = format!("{conversation_ref}/timeline");
    store
        .create_retained_timeline(&conversation_ref, &root_connection_ref)
        .unwrap();
    store
        .retain_invocation_timeline_publication(
            &second_invocation_ref,
            &second.record.source_message_ref,
            &format!("{conversation_ref}/publications/second"),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();

    let first_page = {
        let mut reader = store.read_store().unwrap();
        let page = reader
            .begin_operation(std::time::Duration::from_secs(2))
            .unwrap()
            .conversation_completions(&conversation_ref, root, 0, None, None, 1)
            .unwrap();
        page
    };
    assert_eq!(first_page.items[0].invocation_ref, second_invocation_ref);
    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(operation
        .conversation_completions(
            &conversation_ref,
            root,
            first_page.next_after,
            Some(first_page.through_sequence),
            None,
            1,
        )
        .is_err());
    drop(operation);
    drop(reader);

    store
        .retain_invocation_timeline_publication(
            &first_invocation_ref,
            &first.record.source_message_ref,
            &format!("{conversation_ref}/publications/first"),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    let mut reader = store.read_store().unwrap();
    let error = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap()
        .conversation_completions(
            &conversation_ref,
            root,
            first_page.next_after,
            Some(first_page.through_sequence),
            Some(&first_page.revision),
            1,
        )
        .unwrap_err();
    assert!(error.to_string().contains("earlier canonical timeline"));
}

#[test]
fn legacy_timeline_accepts_a_serialized_position_only_cursor() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy-position-cursor.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let first = store
        .record_host_exchange(
            None,
            crate::v2026_09::test_fixture::actor_ref("person"),
            crate::v2026_09::test_fixture::actor_ref("helper"),
            "first".into(),
            "second".into(),
        )
        .unwrap();
    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    let page = operation
        .conversation_timeline(
            &first.conversation.conversation_ref,
            ConversationRecordWindow {
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    let emitted_cursor = match &page.anchors[0] {
        ConversationTimelineAnchor::Message { key, .. }
        | ConversationTimelineAnchor::Turn { key, .. } => key.clone(),
    };
    let cursor: ConversationPositionKey = serde_json::from_value(serde_json::json!({
        "position": emitted_cursor.position
    }))
    .unwrap();
    assert!(cursor.revision.is_none());
    operation
        .conversation_timeline(
            &first.conversation.conversation_ref,
            ConversationRecordWindow {
                after: Some(&cursor),
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
}

#[test]
fn canonical_flat_cursors_track_topology_not_unrelated_progress() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical-flat-revision.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "canonical-flat-revision");
    let claimed = claim_working(&mut store, &turn);
    let conversation_ref = turn.record.conversation_ref.clone();
    let message_ref = turn.record.source_message_ref.clone();
    let root_connection_ref = format!("{conversation_ref}/timeline");
    store
        .create_retained_timeline(&conversation_ref, &root_connection_ref)
        .unwrap();
    store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/source"),
            &message_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();

    let first_page = {
        let mut reader = store.read_store().unwrap();
        let page = reader
            .begin_operation(std::time::Duration::from_secs(2))
            .unwrap()
            .conversation_timeline(
                &conversation_ref,
                ConversationRecordWindow {
                    first: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
        page
    };
    let cursor = match &first_page.anchors[0] {
        ConversationTimelineAnchor::Message { key, .. }
        | ConversationTimelineAnchor::Turn { key, .. } => key.clone(),
    };

    store
        .record_invocation_progress_with_activity(
            &claimed.adapter_invocation.invocation_ref,
            102,
            "Progress without a placement",
        )
        .unwrap();
    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    let after_progress = operation
        .conversation_timeline(
            &conversation_ref,
            ConversationRecordWindow {
                after: Some(&cursor),
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(after_progress.revision, first_page.revision);
    drop(operation);
    drop(reader);

    store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/source-again"),
            &message_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(operation
        .conversation_timeline(
            &conversation_ref,
            ConversationRecordWindow {
                after: Some(&cursor),
                first: Some(1),
                ..Default::default()
            },
        )
        .is_err());
}
