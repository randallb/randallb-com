#[test]
fn incremental_mutations_refresh_the_indexed_timeline() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("timeline.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let turn = working_turn(&mut store, None, "timeline-incremental");
    let claimed = claim_working(&mut store, &turn);

    let read_timeline = |store: &ConversationAdmissionStore| {
        let mut reader = store.read_store().unwrap();
        let page = reader
            .begin_operation(std::time::Duration::from_secs(2))
            .unwrap()
            .conversation_timeline(
                &turn.record.conversation_ref,
                ConversationRecordWindow {
                    first: Some(10),
                    ..Default::default()
                },
            )
            .unwrap();
        page
    };

    let running = read_timeline(&store);
    assert!(running.anchors.iter().any(|anchor| matches!(
        anchor,
        ConversationTimelineAnchor::Turn { invocation_ref, .. }
            if invocation_ref == &claimed.adapter_invocation.invocation_ref
    )));

    store
        .record_invocation_progress_with_activity(
            &claimed.adapter_invocation.invocation_ref,
            102,
            "Indexed progress",
        )
        .unwrap();
    let progressed = read_timeline(&store);
    assert!(progressed.activity.iter().any(|activity| matches!(
        &activity.event,
        ConversationActivityEvent::WorkUpdate { text: Some(text), .. }
            if text == "Indexed progress"
    )));

    let read_summary = |store: &ConversationAdmissionStore| {
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
            .find(|summary| summary.key.conversation_ref == turn.record.conversation_ref)
            .unwrap();
        summary
    };
    let running_summary = read_summary(&store);

    settle_working(&mut store, &claimed);
    let completed = read_timeline(&store);
    assert!(completed.activity.iter().any(|activity| matches!(
        &activity.event,
        ConversationActivityEvent::WorkLifecycle {
            invocation_ref,
            state: crate::v2026_09::invocation::ActionInvocationState::Completed,
            ..
        } if invocation_ref == &claimed.adapter_invocation.invocation_ref
    )));
    let completed_summary = read_summary(&store);
    assert_eq!(
        completed_summary.completed_work_refs,
        vec![claimed.adapter_invocation.invocation_ref.clone()]
    );
    assert_eq!(completed_summary.completed_work.len(), 1);
    assert_eq!(completed_summary.completed_work[0].invocation_ref, claimed.adapter_invocation.invocation_ref);
    assert!(completed_summary.completed_work[0].sequence > 0);
    let sequence=completed_summary.completed_work[0].sequence;
    drop(store);
    let store=ConversationAdmissionStore::open(&path,"cwp://com.example.runtime").unwrap();
    assert_eq!(read_summary(&store).completed_work[0].sequence,sequence);
    assert!(
        completed_summary.revision.parse::<i64>().unwrap()
            > running_summary.revision.parse::<i64>().unwrap()
    );
}

#[test]
fn reopening_refreshes_an_older_timeline_projection_once() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("timeline-projection.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "timeline-projection-upgrade");
    drop(store);

    let revision_before = cwpdb_store::test_support::timeline_persistence::erase_old_timeline_projection(&path, &turn.record.source_message_ref).unwrap();
    drop(ConversationAdmissionStore::open(&path, root).unwrap());
    let after = cwpdb_store::test_support::timeline_persistence::projection_facts(&path, &turn.record.source_message_ref).unwrap();
    assert_eq!(after.commit, revision_before + 1);
    assert_eq!(after.projection_version, 1);
    assert_eq!(after.source_items, 1);
    drop(ConversationAdmissionStore::open(&path, root).unwrap());
    assert_eq!(cwpdb_store::test_support::timeline_persistence::commit_counter(&path).unwrap(), after.commit);
}

#[test]
fn reopening_and_archive_cycles_keep_bindings_and_unknown_properties() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cwpdb.sqlite3");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "binding-reuse");
    let claimed = claim_working(&mut store, &turn);
    settle_working(&mut store, &claimed);
    let conversation = &turn.record.conversation_ref;
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    store
        .set_conversation_archived(conversation, person, true)
        .unwrap();
    store
        .set_conversation_archived(conversation, person, false)
        .unwrap();
    let expected = cwpdb_store::test_support::timeline_persistence::seed_unknown_properties(&path).unwrap();
    for _ in 0..5 {
        store
            .set_conversation_archived(conversation, person, true)
            .unwrap();
        store
            .set_conversation_archived(conversation, person, false)
            .unwrap();
        drop(store);
        store = ConversationAdmissionStore::open(&path, root).unwrap();
        cwpdb_store::test_support::timeline_persistence::assert_bindings_preserved(&path, &expected).unwrap();
        assert!(!store.conversation(conversation).unwrap().archived);
    }
    cwpdb_store::test_support::timeline_persistence::assert_unknown_properties_preserved(&path).unwrap();
}

#[test]
fn visualization_metadata_reopens_without_changing_the_message_record_contract() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("visualization.sqlite3");
    let root = "cwp://com.example.runtime";
    let mut store =
        ConversationAdmissionStore::open_bound(&path, root, "workspace-a", directory.path())
            .unwrap();
    let turn = working_turn(&mut store, None, "visualization-storage");
    let artifact_ref = format!("{root}/visualization-artifacts/chart");
    store
        .transact(|state| {
            let message = &mut state.conversations[0].messages[0];
            message.text = "Before\n\n\u{e200}visualize\u{e202}{\"path\":\"/private/chart.html\",\"title\":\"Saved chart\"}\u{e201}".into();
            message.visualizations = vec![ConversationVisualization {
                marker_index: 0,
                title: Some("Saved chart".into()),
                mode: None,
                artifact_ref: Some(artifact_ref.clone()),
                error: None,
            }];
            state.visualization_artifacts.insert(
                artifact_ref.clone(),
                ConversationVisualizationArtifact {
                    artifact_ref: artifact_ref.clone(),
                    html: "<p>private bytes</p>".into(),
                },
            );
            Ok(())
        })
        .unwrap();
    assert!(store.state.visualization_artifacts.is_empty());

    let facts = cwpdb_store::test_support::timeline_persistence::visualization_facts(&path, &turn.record.conversation_ref, &turn.record.source_message_ref).unwrap();
    assert!(!facts.message_record.contains("visualizations"));
    assert!(facts.preview.contains("Saved chart"));
    assert!(!facts.preview.contains("/private/chart.html"));
    assert_eq!(facts.visualization_count, 1);
    drop(store);

    let revision_before_rejected_open = cwpdb_store::test_support::timeline_persistence::seed_legacy_visualization_metadata(&path, &turn.record.conversation_ref, &turn.record.source_message_ref, &serde_json::to_value([ConversationVisualization { marker_index:0,title:Some("Saved chart".into()), mode:None,artifact_ref:Some(artifact_ref.clone()),error:None }]).unwrap()).unwrap();

    assert!(ConversationAdmissionStore::open_bound(
        &path,
        root,
        "workspace-b",
        directory.path()
    )
    .is_err());
    assert_eq!(cwpdb_store::test_support::timeline_persistence::commit_counter(&path).unwrap(), revision_before_rejected_open);
    let rejected = cwpdb_store::test_support::timeline_persistence::visualization_facts(&path, &turn.record.conversation_ref, &turn.record.source_message_ref).unwrap();
    assert!(rejected.message_record.contains("visualizations"));

    let reopened =
        ConversationAdmissionStore::open_bound(&path, root, "workspace-a", directory.path())
            .unwrap();
    assert!(reopened.state.visualization_artifacts.is_empty());
    let message = &reopened
        .conversation(&turn.record.conversation_ref)
        .unwrap()
        .messages[0];
    assert_eq!(message.visualizations.len(), 1);
    assert_eq!(
        reopened
            .visualization_artifact(&artifact_ref)
            .unwrap()
            .html,
        "<p>private bytes</p>"
    );
    cwpdb_store::test_support::timeline_persistence::corrupt_visualization_with_oversized_html(&path, super::visualization::MAX_VISUALIZATION_BYTES + 1).unwrap();
    assert!(reopened
        .visualization_artifact(&artifact_ref)
        .unwrap_err()
        .to_string()
        .contains("exceeds the 1 MB limit"));
    cwpdb_store::test_support::timeline_persistence::corrupt_visualization_artifact_reference(&path).unwrap();
    assert!(reopened
        .visualization_artifact(&artifact_ref)
        .unwrap_err()
        .to_string()
        .contains("artifact reference is invalid"));
    let migrated = cwpdb_store::test_support::timeline_persistence::visualization_facts(&path, &turn.record.conversation_ref, &turn.record.source_message_ref).unwrap();
    assert!(!migrated.message_record.contains("visualizations"));
    assert_eq!(migrated.visualization_count,1);
    assert!(migrated.preview.contains("Saved chart"));
    assert!(!migrated.preview.contains("/private/chart.html"));
}

#[test]
fn unsupported_record_schema_is_rejected_before_runtime_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cwpdb.sqlite3");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    working_turn(&mut store, None, "future-record");
    drop(store);
    cwpdb_store::test_support::timeline_persistence::seed_future_invocation_schema(&path).unwrap();
    let error = ConversationAdmissionStore::open(&path, root)
        .err()
        .expect("future invocation rejected");
    assert!(error.to_string().contains("unsupported record schema"));
    cwpdb_store::test_support::timeline_persistence::assert_future_invocation_schema_preserved(&path).unwrap();
}

#[test]
fn summary_generation_ignores_progress_and_advances_for_completion() {
    let directory=tempfile::tempdir().unwrap();
    let mut store=ConversationAdmissionStore::open(directory.path().join("generation.sqlite"),"cwp://com.example.runtime").unwrap();
    let turn=working_turn(&mut store,None,"generation");
    let claimed=claim_working(&mut store,&turn);
    let revision=|store:&ConversationAdmissionStore| {
        let mut reader=store.read_store().unwrap();
        let page=reader.begin_operation(std::time::Duration::from_secs(2)).unwrap().root_conversations(ConversationSummaryWindow{first:Some(1),..Default::default()}).unwrap();
        page.revision
    };
    let before=revision(&store);
    store.record_invocation_progress_with_activity(&claimed.adapter_invocation.invocation_ref,102,"Streaming").unwrap();
    assert_eq!(revision(&store),before);
    settle_working(&mut store,&claimed);
    assert_ne!(revision(&store),before);
}

#[test]
fn completion_summary_is_constant_size_and_history_has_fixed_upper_bound() {
    let directory=tempfile::tempdir().unwrap();
    let path = directory.path().join("completion-pages.sqlite");
    let mut store=ConversationAdmissionStore::open(&path,"cwp://com.example.runtime").unwrap();
    let turn=working_turn(&mut store,None,"completion-pages");
    let conversation=&turn.record.conversation_ref;
    let seed=|_:&ConversationAdmissionStore,sequence:i64| cwpdb_store::test_support::timeline_persistence::seed_legacy_completion(&path,conversation,sequence).unwrap();
    for sequence in 1..=601 {seed(&store,sequence);}
    let read=|store:&ConversationAdmissionStore,after,through| {
        let mut reader=store.read_store().unwrap();
        let page=reader.begin_operation(std::time::Duration::from_secs(2)).unwrap().conversation_completions(conversation,"cwp://com.example.runtime",after,through,None,200).unwrap();
        page
    };
    let first=read(&store,0,None);
    assert_eq!(first.items.len(),200); assert_eq!(first.through_sequence,601); assert!(first.has_next_page);
    seed(&store,602);
    let second=read(&store,first.next_after,Some(first.through_sequence));
    let third=read(&store,second.next_after,Some(first.through_sequence));
    let fourth=read(&store,third.next_after,Some(first.through_sequence));
    assert_eq!(fourth.items.len(),1);assert!(!fourth.has_next_page);assert_eq!(fourth.next_after,601);
    let mut reader=store.read_store().unwrap();
    let operation=reader.begin_operation(std::time::Duration::from_secs(2)).unwrap();
    let summary=operation.root_conversations(ConversationSummaryWindow {first:Some(10),..Default::default()}).unwrap().summaries.into_iter().find(|row|&row.key.conversation_ref==conversation).unwrap();
    assert_eq!(summary.completed_work.len(),1);assert_eq!(summary.completed_work[0].sequence,602);
    assert!(operation.conversation_completions("cwp://com.example.runtime/conversations/missing","cwp://com.example.runtime",0,None,None,200).is_err());
}

#[test]
fn summary_page_revision_tracks_preview_changes_but_not_work_progress() {
    let directory=tempfile::tempdir().unwrap();
    let mut store=ConversationAdmissionStore::open(directory.path().join("preview-revision.sqlite"),"cwp://com.example.runtime").unwrap();
    let turn=working_turn(&mut store,None,"preview-revision");let claimed=claim_working(&mut store,&turn);
    let read=|store:&ConversationAdmissionStore| {let mut reader=store.read_store().unwrap();let page=reader.begin_operation(std::time::Duration::from_secs(2)).unwrap().root_conversations(ConversationSummaryWindow{first:Some(10),..Default::default()}).unwrap();page};
    let first=read(&store);
    store.record_invocation_progress_with_activity(&claimed.adapter_invocation.invocation_ref,102,"Working").unwrap();
    assert_eq!(first.revision,read(&store).revision);
    store.transact(|state| {state.conversations.iter_mut().find(|row|row.conversation_ref==turn.record.conversation_ref).unwrap().messages.last_mut().unwrap().text="Edited preview".into();Ok(())}).unwrap();
    let edited=read(&store);assert_ne!(first.revision,edited.revision);assert_eq!(edited.summaries[0].last_message_preview.as_deref(),Some("Edited preview"));
    store.transact(|state| {let row=state.conversations.iter_mut().find(|row|row.conversation_ref==turn.record.conversation_ref).unwrap();let mut message=row.messages.last().unwrap().clone();message.message_ref="cwp://com.example.runtime/messages/new-preview".into();row.messages.push(message);Ok(())}).unwrap();
    let added=read(&store);assert_ne!(edited.revision,added.revision);assert_ne!(edited.summaries[0].last_message_ref,added.summaries[0].last_message_ref);
    store.transact(|state| {state.conversations.iter_mut().find(|row|row.conversation_ref==turn.record.conversation_ref).unwrap().messages.pop();Ok(())}).unwrap();
    let removed=read(&store);assert_ne!(added.revision,removed.revision);assert_eq!(removed.summaries[0].last_message_ref,edited.summaries[0].last_message_ref);
}
