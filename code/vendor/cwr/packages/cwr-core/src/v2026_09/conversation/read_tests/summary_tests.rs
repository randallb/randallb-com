use super::*;

#[test]
fn workspace_summary_point_read_excludes_retained_pack_conversations() {
    let (_temporary, path) = seeded_store(3);
    let participant = format!("{RUNTIME}/actors/person");
    let workspace_owner = "cwp://example.test/workspaces/one/context";
    let retired_pack_owner = "urn:uuid:11111111-1111-4111-8111-111111111111";
    let workspace_conversation = format!("{RUNTIME}/conversations/00001");
    let pack_conversation = format!("{RUNTIME}/conversations/00002");
    for (reference, owner) in [
        (&workspace_conversation, workspace_owner),
        (&pack_conversation, retired_pack_owner),
    ] {
        cwpdb_store::test_support::summary_pages::owner_context(&path, RUNTIME, reference, owner)
            .unwrap();
    }
    for index in 0..3 {
        cwpdb_store::test_support::summary_pages::participant(
            &path,
            RUNTIME,
            &format!("{RUNTIME}/conversations/{index:05}"),
            &participant,
            true,
        )
        .unwrap();
    }

    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    let legacy_conversation = format!("{RUNTIME}/conversations/00000");
    for reference in [&legacy_conversation, &workspace_conversation] {
        assert!(operation
            .workspace_conversation_summary(reference, Some(workspace_owner), &participant)
            .unwrap()
            .is_some());
    }
    assert!(operation
        .workspace_conversation_summary(&pack_conversation, Some(workspace_owner), &participant)
        .unwrap()
        .is_none());
    assert!(operation
        .workspace_conversation_summary(&legacy_conversation, None, &participant)
        .unwrap()
        .is_some());
    assert!(operation
        .workspace_conversation_summary(&workspace_conversation, None, &participant)
        .unwrap()
        .is_none());
    // Other readers still use the unscoped method after their own authority check.
    assert!(operation
        .root_conversation_summary(&pack_conversation)
        .unwrap()
        .is_some());
    drop(operation);

    let legacy_only = ConversationSummaryPageRequest {
        include_legacy_owner: true,
        first: Some(10),
        ..Default::default()
    };
    let scoped = store
        .begin_operation(TIMEOUT)
        .unwrap()
        .scope_to_owner(&legacy_only);
    let page = scoped
        .participant_root_conversations(&participant, None, legacy_only.window())
        .unwrap();
    assert_eq!(page.total_count, 1);
    assert_eq!(page.summaries[0].key.conversation_ref, legacy_conversation);
    drop(scoped);

    let unfiltered = ConversationSummaryPageRequest {
        first: Some(10),
        ..Default::default()
    };
    let page = store
        .begin_operation(TIMEOUT)
        .unwrap()
        .scope_to_owner(&unfiltered)
        .participant_root_conversations(&participant, None, unfiltered.window())
        .unwrap();
    assert_eq!(page.total_count, 3);

    let snapshot = ConversationPublicationSnapshot::open(
        &path,
        RUNTIME,
        &participant,
        Instant::now() + TIMEOUT,
        ConversationReadCancellation::pending(),
    )
    .unwrap();
    let page = snapshot.conversations(None, &legacy_only).unwrap();
    assert_eq!(page.total_count, 1);
    assert_eq!(page.summaries[0].key.conversation_ref, legacy_conversation);
    assert_eq!(
        snapshot
            .conversations(None, &unfiltered)
            .unwrap()
            .total_count,
        3
    );
}

#[test]
fn workspace_summary_point_read_hides_conversation_after_participation_loss() {
    let (_temporary, path) = seeded_store(1);
    let participant = format!("{RUNTIME}/actors/person");
    let conversation = format!("{RUNTIME}/conversations/00000");
    cwpdb_store::test_support::summary_pages::participant(
        &path,
        RUNTIME,
        &conversation,
        &participant,
        true,
    )
    .unwrap();

    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    assert!(operation
        .workspace_conversation_summary(&conversation, None, &participant)
        .unwrap()
        .is_some());
    drop(operation);
    drop(store);

    cwpdb_store::test_support::summary_pages::participant(
        &path,
        RUNTIME,
        &conversation,
        &participant,
        false,
    )
    .unwrap();
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    assert!(operation
        .workspace_conversation_summary(&conversation, None, &participant)
        .unwrap()
        .is_none());
}

#[test]
fn summaries_include_recent_completed_root_work() {
    let (_temporary, path) = seeded_store(1);
    let conversation = format!("{RUNTIME}/conversations/00000");
    let latest = format!("{RUNTIME}/invocations/latest");
    cwpdb_store::test_support::summary_pages::completed_root_reference(
        &path,
        RUNTIME,
        &conversation,
        &latest,
    )
    .unwrap();

    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    let page = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.summaries[0].completed_work_refs, vec![latest]);
}

#[test]
fn sidebar_filter_preserves_ambiguous_legacy_roots_before_pagination() {
    let (_temporary, path) = seeded_store(4);
    cwpdb_store::test_support::summary_pages::ambiguous_legacy_sidebar(&path, RUNTIME).unwrap();
    let legacy = format!("{RUNTIME}/conversations/00002");
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    let page = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.total_count, 2);
    assert!(page.has_next_page);
    assert_eq!(page.summaries[0].key.conversation_ref, legacy);
    let next = operation
        .root_conversations(ConversationSummaryWindow {
            after: Some(&page.summaries[0].key),
            first: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(next.total_count, 2);
    assert!(!next.has_next_page);
    assert_eq!(
        next.summaries[0].key.conversation_ref,
        format!("{RUNTIME}/conversations/00000")
    );
    // Sidebar filtering must not make standalone CLI history unreadable.
    assert!(operation.require_conversation_revision(&legacy).is_ok());
}

#[test]
fn summary_window_keeps_large_canonical_sources_cold_between_selected_previews() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large-summaries.sqlite");
    drop(ConversationAdmissionStore::open(&path, RUNTIME).unwrap());
    cwpdb_store::test_support::summary_windows::large_canonical_sources(&path, RUNTIME).unwrap();
    let mut reader = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = reader.begin_operation(Duration::from_secs(10)).unwrap();
    let page = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(4),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.total_count, 4);
    assert_eq!(page.summaries.len(), 4);
    for (row, index) in page.summaries.iter().zip((0..4).rev()) {
        assert_eq!(
            row.key.conversation_ref,
            format!("{RUNTIME}/large/conversations/{index}")
        );
        assert_eq!(
            row.last_message_ref.as_deref(),
            Some(format!("{RUNTIME}/large/conversations/{index}/message").as_str())
        );
        let preview = row.last_message_preview.as_ref().unwrap();
        assert_eq!(preview.chars().count(), 240);
        assert!(preview.starts_with(&format!("Source {index} ")));
    }
    assert!(!page.has_next_page);
    assert!(!page.has_previous_page);
}
