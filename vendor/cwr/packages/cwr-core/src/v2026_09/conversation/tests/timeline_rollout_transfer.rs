#[test]
fn enabled_timeline_rollout_rejects_legacy_imports_and_forks_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rollout-transfer.sqlite");
    let root = "cwp://com.example.runtime";
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let ConversationAdmissionOutcome::Started(started) = store
        .admit(
            &graph,
            &ingress(
                "rollout-transfer-source",
                person,
                None,
                ConversationIngressAddressing::Unaddressed,
                "Legacy transfer source",
            ),
            None,
        )
        .unwrap()
    else {
        panic!("source conversation")
    };
    let conversation_ref = started.conversation.conversation_ref;
    let message_ref = started.conversation.messages[0].message_ref.clone();
    let export = store
        .export_conversation(&conversation_ref, person)
        .unwrap();
    store
        .configure_timeline_rollout(ConversationTimelineRollout::NewConversations)
        .unwrap();
    let conversation_count = store.conversations().count();

    let import_error = store
        .import_conversation(&graph, person, export, Some("rollout-import"))
        .unwrap_err();
    assert!(import_error
        .to_string()
        .contains("unavailable while canonical timeline rollout is enabled"));
    assert_eq!(store.conversations().count(), conversation_count);

    let fork_error = store
        .fork_conversation(
            &graph,
            person,
            &conversation_ref,
            &message_ref,
            "rollout-fork",
        )
        .unwrap_err();
    assert!(fork_error
        .to_string()
        .contains("unavailable while canonical timeline rollout is enabled"));
    assert_eq!(store.conversations().count(), conversation_count);
    assert!(store.retained_timeline(&conversation_ref).is_none());
}

#[test]
fn canonical_transfer_round_trip_preserves_publications_and_stops_live_work() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical-export.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        "cwp://com.example.runtime",
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let turn = working_turn(&mut store, None, "export-live-work");
    let claimed = claim_working(&mut store, &turn);
    let source = turn.record.conversation_ref.clone();
    let original = turn.record.invocation_ref.clone().unwrap();
    let export = store.export_conversation(&source, person).unwrap();
    assert_eq!(export.version, "cwr.conversation-export/3");
    let serialized = serde_json::to_string(&export).unwrap();
    assert!(!serialized.contains("actor_integration_ref"));
    assert!(!serialized.contains("attempt_active"));
    let export: ConversationExport = serde_json::from_str(&serialized).unwrap();
    let imported = store
        .import_conversation(&graph, person, export.clone(), Some("portable-timeline"))
        .unwrap();
    let timeline = store
        .retained_timeline(&imported.conversation_ref)
        .unwrap()
        .clone();
    assert_eq!(
        timeline
            .connections
            .iter()
            .map(|c| c.items.len())
            .sum::<usize>(),
        export
            .timeline
            .as_ref()
            .unwrap()
            .timeline
            .connections
            .iter()
            .map(|c| c.items.len())
            .sum::<usize>()
    );
    assert_eq!(imported.messages.len(), export.conversation.messages.len());
    assert_eq!(imported.messages[0].author_actor_ref, person);
    assert_eq!(imported.participant_actor_refs, vec![person.to_string()]);
    let copied = store
        .invocation_records()
        .find(|r| r.conversation_ref.as_deref() == Some(&imported.conversation_ref))
        .unwrap();
    assert_eq!(
        copied.state,
        crate::v2026_09::invocation::ActionInvocationState::Stopped
    );
    assert!(!copied.is_runnable());
    assert!(!copied.is_live_root_completion());
    assert!(copied.actor_integration_ref.is_none());
    assert_eq!(copied.input, serde_json::Value::Null);
    assert_eq!(
        store.invocation_record(&original).unwrap().state,
        crate::v2026_09::invocation::ActionInvocationState::Running
    );
    settle_working(&mut store, &claimed);
    let completed = store.export_conversation(&source, person).unwrap();
    assert!(completed.timeline.as_ref().unwrap().work[0]
        .status
        .is_completed());
    let completed_import = store
        .import_conversation(&graph, person, completed, Some("portable-completed"))
        .unwrap();
    assert!(store
        .invocation_records()
        .filter(|r| r.conversation_ref.as_deref() == Some(&completed_import.conversation_ref))
        .all(|r| !r.is_live_root_completion()));
    let second_export = store
        .export_conversation(&imported.conversation_ref, person)
        .unwrap();
    assert!(second_export.timeline.as_ref().unwrap().work[0]
        .status
        .is_stopped());
    drop(store);
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        "cwp://com.example.runtime",
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    assert_eq!(
        store.retained_timeline(&imported.conversation_ref).unwrap(),
        &timeline
    );
    assert_eq!(
        store
            .import_conversation(&graph, person, export, Some("portable-timeline"))
            .unwrap(),
        imported
    );
    let new_turn = working_turn(&mut store, Some(&imported.conversation_ref), "after-import");
    let new_claim = claim_working(&mut store, &new_turn);
    settle_working(&mut store, &new_claim);
}

#[test]
fn canonical_transfer_rejects_invalid_publications_without_writing() {
    let directory = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        directory.path().join("invalid-export.sqlite"),
        "cwp://com.example.runtime",
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let turn = working_turn(&mut store, None, "export-invalid");
    let export = store
        .export_conversation(&turn.record.conversation_ref, person)
        .unwrap();
    let count = store.conversations().count();
    let mut missing = export.clone();
    missing.timeline.as_mut().unwrap().work.clear();
    assert!(store
        .import_conversation(&graph, person, missing, Some("invalid-missing"))
        .is_err());
    let mut live = export.clone();
    live.timeline.as_mut().unwrap().work[0].status = ActionResponseStatus::PROGRESS;
    assert!(store
        .import_conversation(&graph, person, live, Some("invalid-live"))
        .is_err());
    let mut version = export.clone();
    version.version = "cwr.conversation-export/2".into();
    assert!(store
        .import_conversation(&graph, person, version, Some("invalid-version"))
        .is_err());
    let mut cycle = export.clone();
    let record = &mut cycle.timeline.as_mut().unwrap().work[0];
    record.parent_invocation_ref = Some(record.invocation_ref.clone());
    assert!(store
        .import_conversation(&graph, person, cycle, Some("invalid-cycle"))
        .is_err());
    let mut topology = export;
    topology.timeline.as_mut().unwrap().timeline.connections[0].owner =
        ConversationTimelineConnectionOwner::Conversation {
            conversation_ref: "cwp://other/conversation".into(),
        };
    assert!(store
        .import_conversation(&graph, person, topology, Some("invalid-topology"))
        .is_err());
    assert_eq!(store.conversations().count(), count);
}

#[test]
fn canonical_transfer_preserves_nested_repeated_placements_streams_and_inspector() {
    let directory = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        directory.path().join("nested-export.sqlite"),
        "cwp://com.example.runtime",
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let turn = working_turn(&mut store, None, "nested-export");
    let claimed = claim_working(&mut store, &turn);
    let source = &turn.record.conversation_ref;
    let invocation = turn.record.invocation_ref.as_ref().unwrap();
    inspector_tool(
        &mut store,
        invocation,
        "exported-tool",
        true,
        "Public tool output",
    );
    let stream = stream_output(
        &claimed,
        "export-stream",
        crate::open_responses::v2026_04_24::AssistantPhase::Commentary,
        1,
        "Published partial reply",
    );
    store.record_streaming_output(Some(&stream), &[]).unwrap();
    let parent = store
        .retained_timeline(source)
        .unwrap()
        .connections
        .iter()
        .flat_map(|c| &c.items)
        .find(|item| item.content.reference() == invocation)
        .unwrap()
        .item_ref
        .clone();
    store.timeline_rollout = None;
    store
        .retain_invocation_timeline_publication(
            invocation,
            &turn.record.source_message_ref,
            "cwp://com.example.runtime/publications/export-repeat",
            ConversationTimelineDisposition::Child {
                parent_item_ref: parent,
            },
        )
        .unwrap();
    let export = store.export_conversation(source, person).unwrap();
    assert_eq!(export.timeline.as_ref().unwrap().work.len(), 1);
    assert_eq!(
        export.timeline.as_ref().unwrap().work[0].activities.len(),
        1
    );
    let destination_path = directory.path().join("repeated-destination.sqlite");
    let mut destination = ConversationAdmissionStore::open_with_timeline_rollout(
        &destination_path,
        "cwp://com.example.destination",
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let imported = destination
        .import_conversation(&graph, person, export, Some("nested-import"))
        .unwrap();
    let partial = imported
        .messages
        .iter()
        .find(|m| m.text == "Published partial reply")
        .unwrap();
    assert_eq!(
        partial.reply_to_message_ref.as_deref(),
        Some(imported.messages[0].message_ref.as_str())
    );
    let copy = destination
        .invocation_records()
        .find(|r| r.conversation_ref.as_deref() == Some(&imported.conversation_ref))
        .unwrap();
    assert_eq!(copy.timeline_publications.len(), 2);
    let timeline = destination
        .retained_timeline(&imported.conversation_ref)
        .unwrap();
    assert_eq!(timeline.connections.len(), 3);
    assert_eq!(
        timeline
            .connections
            .iter()
            .flat_map(|c| &c.items)
            .filter(|i| i.content.reference() == copy.invocation_ref)
            .count(),
        2
    );
    let snapshot = inspector_snapshot(&destination, person);
    let page = snapshot
        .inspector_page(
            &copy.invocation_ref,
            ConversationInspectorCollection::Activities,
            &Default::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(
        snapshot
            .inspector_text(
                &copy.invocation_ref,
                &page.entries[0].0.reference,
                None,
                ConversationInspectorTextField::Output
            )
            .unwrap()
            .unwrap()
            .text,
        "Public tool output"
    );
    drop(snapshot);
    let second = destination
        .export_conversation(&imported.conversation_ref, person)
        .unwrap();
    assert_eq!(
        second.timeline.as_ref().unwrap().work[0].activities.len(),
        1
    );
    destination
        .import_conversation(&graph, person, second, Some("second-nested-import"))
        .unwrap();
    assert_eq!(store.state.streaming_messages.len(), 1);
    assert!(destination.state.streaming_messages.is_empty());
    drop(destination);
    let mut destination = ConversationAdmissionStore::open_with_timeline_rollout(
        &destination_path,
        "cwp://com.example.destination",
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let continued = working_turn(
        &mut destination,
        Some(&imported.conversation_ref),
        "repeated-import-continued",
    );
    let claimed = claim_working(&mut destination, &continued);
    settle_working(&mut destination, &claimed);
}

#[test]
fn canonical_transfer_to_another_runtime_copies_only_disclosed_history() {
    let directory = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let reviewer = "cwp://com.example.fixture/actors/reviewer";
    let mut source = ConversationAdmissionStore::open(
        directory.path().join("source.sqlite"),
        "cwp://com.example.source",
    )
    .unwrap();
    let turn = working_turn(&mut source, None, "undisclosed-export-source");
    let conversation = &turn.record.conversation_ref;
    let invocation = turn.record.invocation_ref.as_ref().unwrap();
    let hidden_text = source.conversation(conversation).unwrap().messages[0]
        .text
        .clone();
    source
        .create_retained_timeline(conversation, &format!("{conversation}/timeline"))
        .unwrap();
    source
        .retain_invocation_timeline_publication(
            invocation,
            &turn.record.source_message_ref,
            &format!("{conversation}/publications/work"),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    assert!(source.export_conversation(conversation, reviewer).is_err());
    let export = source.export_conversation(conversation, person).unwrap();
    assert!(export.conversation.messages.is_empty());
    assert!(export.timeline.as_ref().unwrap().work[0]
        .source_message_ref
        .is_none());
    assert!(!serde_json::to_string(&export)
        .unwrap()
        .contains(&hidden_text));
    let mut destination = ConversationAdmissionStore::open_with_timeline_rollout(
        directory.path().join("destination.sqlite"),
        "cwp://com.example.destination",
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let imported = destination
        .import_conversation(&graph, reviewer, export, Some("other-runtime"))
        .unwrap();
    assert_eq!(imported.participant_actor_refs, vec![reviewer.to_string()]);
    assert!(imported
        .conversation_ref
        .starts_with("cwp://com.example.destination/"));
    let copy = destination.invocation_records().next().unwrap();
    assert!(copy.source_message_ref.is_none());
    assert!(!copy.is_runnable());
    assert_ne!(copy.invocation_ref, *invocation);
    assert_eq!(
        source.invocation_record(invocation).unwrap().state,
        crate::v2026_09::invocation::ActionInvocationState::Admitted
    );
}
