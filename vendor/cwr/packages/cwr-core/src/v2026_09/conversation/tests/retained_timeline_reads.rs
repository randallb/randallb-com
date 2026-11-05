fn populated_canonical_timeline() -> (
    tempfile::TempDir,
    ConversationAdmissionStore,
    String,
    String,
    String,
    String,
    ConversationTimelineItem,
) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical-timeline-read.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "canonical-timeline-read");
    let conversation_ref = turn.record.conversation_ref.clone();
    let message_ref = turn.record.source_message_ref.clone();
    let invocation_ref = turn.record.invocation_ref.clone().unwrap();
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
    let invocation_item = match store
        .retain_invocation_timeline_publication(
            &invocation_ref,
            &message_ref,
            &format!("{conversation_ref}/publications/invocation"),
            ConversationTimelineDisposition::Root,
        )
        .unwrap()
    {
        ConversationTimelinePublishOutcome::Placed(item) => item,
        outcome => panic!("unexpected publication outcome {outcome:?}"),
    };
    store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/child"),
            &message_ref,
            ConversationTimelineDisposition::Child {
                parent_item_ref: invocation_item.item_ref.clone(),
            },
        )
        .unwrap();
    store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/source-again"),
            &message_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();

    (
        directory,
        store,
        conversation_ref,
        message_ref,
        invocation_ref,
        root_connection_ref,
        invocation_item,
    )
}

#[test]
fn canonical_connection_reads_page_only_the_requested_siblings() {
    let (
        _directory,
        mut store,
        conversation_ref,
        message_ref,
        invocation_ref,
        root_connection_ref,
        invocation_item,
    ) = populated_canonical_timeline();

    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    let first = operation
        .conversation_timeline_connection(
            &conversation_ref,
            &root_connection_ref,
            &ConversationTimelinePageRequest {
                first: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(first.revision, 3);
    assert_eq!(first.edges.len(), 2);
    assert!(first.page_info.has_next_page);
    assert!(!first.page_info.has_previous_page);
    assert!(matches!(
        first.owner,
        ConversationTimelineConnectionOwner::Conversation {
            conversation_ref: ref owner
        }
            if owner == &conversation_ref
    ));
    assert!(matches!(
        first.edges[0].item.content,
        ConversationTimelineContent::Message {
            message_ref: ref item
        }
            if item == &message_ref
    ));
    assert!(matches!(
        first.edges[1].item.content,
        ConversationTimelineContent::Invocation {
            invocation_ref: ref item
        }
            if item == &invocation_ref
    ));

    let after = first.page_info.end_cursor.clone().unwrap();
    let second = operation
        .conversation_timeline_connection(
            &conversation_ref,
            &root_connection_ref,
            &ConversationTimelinePageRequest {
                after: Some(after),
                first: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(second.edges.len(), 1);
    assert_eq!(second.edges[0].item.sibling_sequence, 3);
    assert!(!second.page_info.has_next_page);
    assert!(!second.page_info.has_previous_page);

    let newest = operation
        .conversation_timeline_connection(
            &conversation_ref,
            &root_connection_ref,
            &ConversationTimelinePageRequest {
                last: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(newest.edges[0].item.sibling_sequence, 3);
    assert!(newest.page_info.has_previous_page);
    assert!(!newest.page_info.has_next_page);
    let before = newest.page_info.start_cursor.clone().unwrap();
    let previous = operation
        .conversation_timeline_connection(
            &conversation_ref,
            &root_connection_ref,
            &ConversationTimelinePageRequest {
                before: Some(before),
                last: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(previous.edges[0].item.sibling_sequence, 2);
    assert!(previous.page_info.has_previous_page);
    assert!(!previous.page_info.has_next_page);

    let child_connection_ref = invocation_item.child_connection_ref.unwrap();
    let child = operation
        .conversation_timeline_connection(
            &conversation_ref,
            &child_connection_ref,
            &ConversationTimelinePageRequest {
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(child.edges.len(), 1);
    assert!(matches!(
        child.owner,
        ConversationTimelineConnectionOwner::TimelineItem { ref item_ref }
            if item_ref == &invocation_item.item_ref
    ));
    assert_eq!(
        child.edges[0].item.connection_ref,
        child_connection_ref
    );
    for edge in first.edges.iter().chain(&child.edges) {
        let resource = operation
            .conversation_resource(edge.item.content.reference())
            .unwrap();
        assert!(resource.is_some());
    }
    let history = operation
        .conversation_history(
            &conversation_ref,
            ConversationRecordWindow {
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(history.total_count, 1);
    assert_eq!(history.content[0].message.message_ref, message_ref);
    let around = operation
        .conversation_history_around(&conversation_ref, &message_ref, 1, 1, 256)
        .unwrap();
    assert!(around.revision.starts_with("canonical:"));
    assert_eq!(
        around.content[0].key.revision.as_deref(),
        Some(around.revision.as_str())
    );
    assert!(operation
        .conversation_timeline_connection(
            &conversation_ref,
            "cwp://com.example.runtime/other/timeline",
            &ConversationTimelinePageRequest {
                first: Some(1),
                ..Default::default()
            },
        )
        .is_err());

    let tail_cursor = second.page_info.end_cursor.unwrap();
    drop(operation);
    drop(reader);
    store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/source-third"),
            &message_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/child-second"),
            &message_ref,
            ConversationTimelineDisposition::Child {
                parent_item_ref: invocation_item.item_ref,
            },
        )
        .unwrap();
    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    let appended = operation
        .conversation_timeline_connection(
            &conversation_ref,
            &root_connection_ref,
            &ConversationTimelinePageRequest {
                after: Some(tail_cursor),
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(appended.revision, 4);
    assert_eq!(appended.edges.len(), 1);
    assert_eq!(appended.edges[0].item.sibling_sequence, 4);
    let child = operation
        .conversation_timeline_connection(
            &conversation_ref,
            &child_connection_ref,
            &ConversationTimelinePageRequest {
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(child.revision, 2);
    assert_eq!(child.edges.len(), 2);
}

#[test]
fn explicit_empty_timeline_does_not_fall_back_to_legacy_projection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("empty-canonical-timeline.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "empty-canonical-timeline");
    let conversation_ref = turn.record.conversation_ref.clone();
    let message_ref = turn.record.source_message_ref.clone();
    let invocation_ref = turn.record.invocation_ref.clone().unwrap();
    let root_connection_ref = format!("{conversation_ref}/timeline");

    let legacy = {
        let mut reader = store.read_store().unwrap();
        let page = reader
            .begin_operation(std::time::Duration::from_secs(2))
            .unwrap()
            .conversation_timeline(
                &conversation_ref,
                ConversationRecordWindow {
                    first: Some(10),
                    ..Default::default()
                },
            )
            .unwrap();
        page
    };
    assert!(!legacy.anchors.is_empty());

    store
        .create_retained_timeline(&conversation_ref, &root_connection_ref)
        .unwrap();
    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    let canonical = operation
        .conversation_timeline(
            &conversation_ref,
            ConversationRecordWindow {
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(canonical.anchors.is_empty());
    assert!(canonical.messages.is_empty());
    assert!(canonical.activity.is_empty());
    assert_eq!(canonical.total_count, 0);
    assert!(canonical.revision.starts_with("canonical:"));

    let content = operation
        .conversation_content(
            &conversation_ref,
            ConversationRecordWindow {
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(content.content.is_empty());
    assert_eq!(content.total_count, 0);
    let history = operation
        .conversation_history(
            &conversation_ref,
            ConversationRecordWindow {
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(history.content.is_empty());
    assert_eq!(history.total_count, 0);
    let browse = operation
        .conversation_browse_page(
            &conversation_ref,
            ConversationRecordWindow {
                first: Some(10),
                ..Default::default()
            },
            256,
        )
        .unwrap();
    assert!(browse.content.is_empty());
    assert!(operation
        .conversation_history_search(&conversation_ref, "empty", None, None, 10, 256)
        .unwrap()
        .content
        .is_empty());
    assert!(operation
        .conversation_history_around(&conversation_ref, &message_ref, 1, 1, 256)
        .is_err());
    assert!(operation
        .conversation_message_excerpt(&conversation_ref, &message_ref, 0, 0, 256)
        .is_err());
    assert!(operation
        .conversation_message_links(&conversation_ref, &message_ref, None, None, 10)
        .is_err());
    assert!(operation
        .conversation_resource(&message_ref)
        .unwrap()
        .is_none());
    assert!(operation
        .conversation_resource(&invocation_ref)
        .unwrap()
        .is_none());
    let current = operation
        .conversation_current(&conversation_ref)
        .unwrap();
    assert_eq!(current.active_invocations.len(), 1);
    assert_eq!(current.active_invocations[0].invocation_ref, invocation_ref);
    let summary = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(10),
            ..Default::default()
        })
        .unwrap()
        .summaries
        .into_iter()
        .find(|summary| summary.key.conversation_ref == conversation_ref)
        .unwrap();
    assert_eq!(summary.last_message_ref, None);
    assert_eq!(summary.last_message_preview, None);
    assert!(summary.completed_work.is_empty());
    assert!(summary.completed_work_refs.is_empty());
    assert!(summary.revision.starts_with("canonical:"));

    drop(operation);
    drop(reader);
    store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/source"),
            &message_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    let canonical = operation
        .conversation_timeline(
            &conversation_ref,
            ConversationRecordWindow {
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(canonical.anchors.len(), 1);
    assert_eq!(canonical.messages.len(), 1);
    assert!(canonical.activity.iter().all(|activity| !matches!(
        &activity.event,
        ConversationActivityEvent::WorkLifecycle { .. }
            | ConversationActivityEvent::WorkUpdate { .. }
    )));
    assert!(operation
        .conversation_resource(&message_ref)
        .unwrap()
        .is_some());
    assert!(operation
        .conversation_resource(&invocation_ref)
        .unwrap()
        .is_none());
}

#[test]
fn canonical_completion_reads_only_placed_invocations() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical-completion-visibility.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let first = working_turn(&mut store, None, "visible-completion");
    let first_claimed = claim_working(&mut store, &first);
    settle_working(&mut store, &first_claimed);
    let second = working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "hidden-completion",
    );
    let second_claimed = claim_working(&mut store, &second);
    settle_working(&mut store, &second_claimed);
    let conversation_ref = first.record.conversation_ref.clone();
    let first_invocation_ref = first.record.invocation_ref.clone().unwrap();
    let second_invocation_ref = second.record.invocation_ref.clone().unwrap();
    let root_connection_ref = format!("{conversation_ref}/timeline");
    store
        .create_retained_timeline(&conversation_ref, &root_connection_ref)
        .unwrap();
    store
        .retain_invocation_timeline_publication(
            &first_invocation_ref,
            &first.record.source_message_ref,
            &format!("{conversation_ref}/publications/visible-completion"),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    store
        .retain_invocation_timeline_publication(
            &second_invocation_ref,
            &second.record.source_message_ref,
            &format!("{conversation_ref}/publications/hidden-completion"),
            ConversationTimelineDisposition::Undisclosed,
        )
        .unwrap();

    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    let completions = operation
        .conversation_completions(&conversation_ref, root, 0, None, None, 10)
        .unwrap();
    assert_eq!(completions.items.len(), 1);
    assert_eq!(completions.items[0].invocation_ref, first_invocation_ref);
    let summary = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(10),
            ..Default::default()
        })
        .unwrap()
        .summaries
        .into_iter()
        .find(|summary| summary.key.conversation_ref == conversation_ref)
        .unwrap();
    assert_eq!(summary.completed_work_refs, vec![first_invocation_ref.clone()]);
    assert_eq!(summary.completed_work, completions.items);
    assert!(operation
        .conversation_resource(&first_invocation_ref)
        .unwrap()
        .is_some());
    assert!(operation
        .conversation_resource(&second_invocation_ref)
        .unwrap()
        .is_none());
}
