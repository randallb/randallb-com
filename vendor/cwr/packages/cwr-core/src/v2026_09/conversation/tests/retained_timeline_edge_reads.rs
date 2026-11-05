#[test]
fn canonical_invocation_resource_omits_an_unplaced_latest_reply() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical-resource-reply.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "canonical-resource-reply");
    let conversation_ref = turn.record.conversation_ref.clone();
    let invocation_ref = turn.record.invocation_ref.clone().unwrap();
    let timeline_ref = format!("{conversation_ref}/timeline");
    store
        .create_retained_timeline(&conversation_ref, &timeline_ref)
        .unwrap();
    store
        .retain_invocation_timeline_publication(
            &invocation_ref,
            &turn.record.source_message_ref,
            &format!("{conversation_ref}/publications/invocation"),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    let claimed = claim_working(&mut store, &turn);
    settle_working(&mut store, &claimed);
    let reply_ref = store
        .admission_record_for_invocation(&invocation_ref)
        .unwrap()
        .reply_message_ref
        .clone()
        .expect("completion publishes a reply");

    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(operation
        .conversation_invocation(&conversation_ref, &invocation_ref)
        .unwrap()
        .unwrap()
        .latest_reply
        .is_some());
    let resource = operation
        .conversation_resource(&invocation_ref)
        .unwrap()
        .expect("placed invocation remains visible");
    let ConversationResource::Invocation { invocation } = resource.resource else {
        panic!("expected invocation resource");
    };
    assert_eq!(invocation.latest_reply, None);
    assert!(operation
        .conversation_resource(&reply_ref)
        .unwrap()
        .is_none());
}

#[test]
fn summary_page_revision_tracks_canonical_visibility_changes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical-summary-page-revision.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "canonical-summary-page-revision");
    let conversation_ref = turn.record.conversation_ref.clone();
    let timeline_ref = format!("{conversation_ref}/timeline");
    store
        .create_retained_timeline(&conversation_ref, &timeline_ref)
        .unwrap();

    let read = |store: &ConversationAdmissionStore, first| {
        let mut reader = store.read_store().unwrap();
        let page = reader
            .begin_operation(std::time::Duration::from_secs(2))
            .unwrap()
            .root_conversations(ConversationSummaryWindow {
                first: Some(first),
                ..Default::default()
            })
            .unwrap();
        page
    };
    let hidden = read(&store, 10);
    assert_eq!(hidden.summaries[0].last_message_ref, None);
    store
        .publish_timeline_message(
            &conversation_ref,
            &format!("{conversation_ref}/publications/source"),
            &turn.record.source_message_ref,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();

    let visible = read(&store, 10);
    assert_ne!(hidden.revision, visible.revision);
    assert_eq!(
        visible.summaries[0].last_message_ref.as_deref(),
        Some(turn.record.source_message_ref.as_str())
    );
    let revision_only = read(&store, 0);
    assert!(revision_only.summaries.is_empty());
    assert_eq!(revision_only.revision, visible.revision);
}

#[test]
fn canonical_completed_work_keeps_only_the_newest_five_hundred_refs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical-completion-cap.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "canonical-completion-cap");
    let conversation_ref = turn.record.conversation_ref.clone();
    let timeline_ref = format!("{conversation_ref}/timeline");
    store
        .create_retained_timeline(&conversation_ref, &timeline_ref)
        .unwrap();

    cwpdb_store::test_support::timeline_persistence::seed_canonical_completion_history(&path, root, &conversation_ref, &timeline_ref).unwrap();

    let mut reader = store.read_store().unwrap();
    let summary = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap()
        .root_conversations(ConversationSummaryWindow {
            first: Some(10),
            ..Default::default()
        })
        .unwrap()
        .summaries
        .into_iter()
        .find(|summary| summary.key.conversation_ref == conversation_ref)
        .unwrap();
    assert_eq!(summary.completed_work_refs.len(), 500);
    assert_eq!(
        summary.completed_work_refs.first().unwrap(),
        &format!("{root}/invocations/canonical-history-2")
    );
    assert_eq!(
        summary.completed_work_refs.last().unwrap(),
        &format!("{root}/invocations/canonical-history-501")
    );
    assert_eq!(summary.completed_work.len(), 1);
    assert_eq!(summary.completed_work[0].sequence, 501);
}
