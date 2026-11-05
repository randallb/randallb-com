#[test]
fn canonical_fork_freezes_message_prefix_and_recovers_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical-fork.sqlite");
    let root = "cwp://com.example.runtime";
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(&path, root, ConversationTimelineRollout::NewConversations).unwrap();
    store.configure_timeline_rollout(ConversationTimelineRollout::NewConversations).unwrap();
    let ConversationAdmissionOutcome::Started(first) = store.admit(&graph, &ingress("fork-first", person, None, ConversationIngressAddressing::Unaddressed, "First"), None).unwrap() else { panic!("first") };
    let source = first.record.conversation_ref.clone();
    let selected = first.record.source_message_ref.clone();
    store.admit(&graph, &ingress("fork-later", person, Some(&source), ConversationIngressAddressing::Unaddressed, "Later"), None).unwrap();
    let fork = store.fork_conversation(&graph, person, &source, &selected, "canonical-fork").unwrap();
    assert_eq!(fork.messages.len(), 1);
    assert_eq!(fork.messages[0].text, "First");
    assert_eq!(fork.messages[0].author_actor_ref, person);
    assert_eq!(fork.participant_actor_refs, vec![person.to_string()]);
    assert_ne!(fork.messages[0].message_ref, selected);
    let timeline = store.retained_timeline(&fork.conversation_ref).unwrap().clone();
    assert_eq!(timeline.connections.iter().map(|c| c.items.len()).sum::<usize>(), 1);
    assert_ne!(timeline.root_connection_ref, store.retained_timeline(&source).unwrap().root_connection_ref);
    store.admit(&graph, &ingress("fork-source-continues", person, Some(&source), ConversationIngressAddressing::Unaddressed, "Source continues"), None).unwrap();
    assert_eq!(store.conversation(&fork.conversation_ref).unwrap().messages.len(), 1);
    drop(store);
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(&path, root, ConversationTimelineRollout::NewConversations).unwrap();
    let retry = store.fork_conversation(&graph, person, &source, &selected, "canonical-fork").unwrap();
    assert_eq!(retry, fork);
    assert_eq!(store.retained_timeline(&fork.conversation_ref).unwrap(), &timeline);
    let other = store.conversation(&source).unwrap().messages.last().unwrap().message_ref.clone();
    assert!(store.fork_conversation(&graph, person, &source, &other, "canonical-fork").is_err());
}

#[test]
fn canonical_fork_retains_unfinished_work_without_execution_authority() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("canonical-work-fork.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(&path, "cwp://com.example.runtime", ConversationTimelineRollout::NewConversations).unwrap();
    let turn = working_turn(&mut store, None, "fork-working");
    let claimed = claim_working(&mut store, &turn);
    let source = turn.record.conversation_ref.clone();
    let original = turn.record.invocation_ref.clone().unwrap();
    inspector_tool(&mut store, &original, "fork-command", false, "before boundary");
    let ConversationAdmissionOutcome::Started(boundary) = store.admit(&graph, &ingress("fork-work-boundary", person, Some(&source), ConversationIngressAddressing::Unaddressed, "Branch here"), None).unwrap() else { panic!("boundary") };
    inspector_tool(&mut store, &original, "fork-command", true, "after boundary");
    let fork = store.fork_conversation(&graph, person, &source, &boundary.record.source_message_ref, "canonical-work-fork").unwrap();
    let copies = store.invocation_records().filter(|r| r.conversation_ref.as_deref() == Some(&fork.conversation_ref)).collect::<Vec<_>>();
    assert_eq!(copies.len(), 1);
    let copy = copies[0];
    assert_eq!(copy.forked_from_invocation_ref.as_deref(), Some(original.as_str()));
    assert_eq!(copy.state, crate::v2026_09::invocation::ActionInvocationState::Stopped);
    assert!(copy.actor_integration_ref.is_none());
    assert!(!copy.is_runnable());
    let copied_ref = copy.invocation_ref.clone();
    let snapshot = inspector_snapshot(&store, person);
    let activities = snapshot.inspector_page(&copied_ref, ConversationInspectorCollection::Activities, &Default::default()).unwrap().unwrap();
    assert_eq!(activities.entries.len(), 1);
    let activity_ref = &activities.entries[0].0.reference;
    let output = snapshot.inspector_text(&copied_ref, activity_ref, None, ConversationInspectorTextField::Output).unwrap().unwrap();
    assert_eq!(output.text, "before boundary");
    assert!(matches!(&activities.entries[0].1, ConversationInspectorEntry::Activity { active: false, .. }));
    drop(snapshot);
    assert_eq!(store.invocation_record(&original).unwrap().state, crate::v2026_09::invocation::ActionInvocationState::Running);
    assert_eq!(claimed.adapter_invocation.invocation_ref, original);
    let lineage = fork.activity.iter().find_map(|item| match &item.event {
        ConversationActivityEvent::ConversationForkedFrom { inherited_refs, .. } => Some(inherited_refs),
        _ => None,
    }).unwrap();
    assert!(lineage.values().any(|reference| reference == &original));
    assert!(lineage.values().any(|reference| reference == &boundary.record.source_message_ref));
    let expected = lineage.clone();
    drop(store);
    let store = ConversationAdmissionStore::open_with_timeline_rollout(&path, "cwp://com.example.runtime", ConversationTimelineRollout::NewConversations).unwrap();
    assert!(store.conversation(&fork.conversation_ref).unwrap().activity.iter().any(|item| matches!(&item.event, ConversationActivityEvent::ConversationForkedFrom { inherited_refs, .. } if inherited_refs == &expected)));
    let snapshot = inspector_snapshot(&store, person);
    assert_eq!(snapshot.inspector_text(&copied_ref, activity_ref, None, ConversationInspectorTextField::Output).unwrap().unwrap().text, "before boundary");
}

#[test]
fn canonical_fork_history_never_creates_completion_attention() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fork-attention.sqlite");
    let root = "cwp://com.example.runtime";
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path, root, ConversationTimelineRollout::NewConversations,
    ).unwrap();
    let turn = working_turn(&mut store, None, "completed-source");
    let claimed = claim_working(&mut store, &turn);
    settle_working(&mut store, &claimed);
    let source = store.conversation(&turn.record.conversation_ref).unwrap();
    let selected = source.messages.last().unwrap().message_ref.clone();
    let fork = store.fork_conversation(&graph, person, &turn.record.conversation_ref, &selected, "attention-fork").unwrap();
    let copy = store.invocation_records().find(|r| r.conversation_ref.as_deref() == Some(&fork.conversation_ref)).unwrap();
    assert_eq!(copy.state, crate::v2026_09::invocation::ActionInvocationState::Completed);
    assert!(!copy.is_live_root_completion());
    assert!(!copy.is_runnable());
    assert!(!store.retained_timeline(&fork.conversation_ref).unwrap().connections.is_empty());
    let read_completions = |store: &ConversationAdmissionStore, conversation: &str| {
        let mut reader = store.read_store().unwrap();
        let operation = reader.begin_operation(std::time::Duration::from_secs(2)).unwrap();
        let items = operation.conversation_completions(conversation, root, 0, None, None, 10).unwrap().items;
        let summary = operation.root_conversations(ConversationSummaryWindow { first: Some(10), ..Default::default() }).unwrap().summaries.into_iter().find(|s| s.key.conversation_ref == conversation).unwrap();
        assert_eq!(summary.completed_work, items);
        assert_eq!(summary.completed_work_refs, items.iter().map(|item| item.invocation_ref.clone()).collect::<Vec<_>>());
        items
    };
    let source_completions = read_completions(&store, &turn.record.conversation_ref);
    assert_eq!(source_completions.len(), 1);
    assert!(read_completions(&store, &fork.conversation_ref).is_empty());
    drop(store);
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path, root, ConversationTimelineRollout::NewConversations,
    ).unwrap();
    assert!(read_completions(&store, &fork.conversation_ref).is_empty());
    let live = working_turn(&mut store, Some(&fork.conversation_ref), "new-fork-work");
    let claimed = claim_working(&mut store, &live);
    settle_working(&mut store, &claimed);
    let completions = read_completions(&store, &fork.conversation_ref);
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].sequence, source_completions[0].sequence + 1);
    assert_eq!(completions[0].invocation_ref, claimed.adapter_invocation.invocation_ref);
    drop(store);
    let store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path, root, ConversationTimelineRollout::NewConversations,
    ).unwrap();
    assert_eq!(read_completions(&store, &fork.conversation_ref), completions);
}

#[test]
fn canonical_fork_omits_associations_for_undisclosed_replies() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fork-undisclosed.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let turn = working_turn(&mut store, None, "private-work");
    let source = &turn.record.conversation_ref;
    store.create_retained_timeline(source, &format!("{source}/timeline")).unwrap();
    store.publish_timeline_message(source, &format!("{source}/publications/source"), &turn.record.source_message_ref, ConversationTimelineDisposition::Root).unwrap();
    store.retain_invocation_timeline_publication(turn.record.invocation_ref.as_deref().unwrap(), &turn.record.source_message_ref, &format!("{source}/publications/private"), ConversationTimelineDisposition::Undisclosed).unwrap();
    store.configure_timeline_rollout(ConversationTimelineRollout::NewConversations).unwrap();
    let claimed = claim_working(&mut store, &turn);
    settle_working(&mut store, &claimed);
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let ConversationAdmissionOutcome::Started(boundary) = store.admit(&graph, &ingress("published-boundary", person, Some(source), ConversationIngressAddressing::Unaddressed, "Branch here"), None).unwrap() else { panic!("boundary") };
    let fork = store.fork_conversation(&graph, person, source, &boundary.record.source_message_ref, "filtered-associations").unwrap();
    assert_eq!(fork.messages.len(), 2);
    assert!(!fork.messages.iter().any(|m| m.text == "Done"));
    assert!(fork.activity.iter().all(|item| match &item.event {
        ConversationActivityEvent::MessageWorkAssociation { message_ref, .. } => fork.messages.iter().any(|m| &m.message_ref == message_ref),
        _ => true,
    }));
}

#[test]
fn canonical_fork_rejects_ambiguous_repeated_placements() {
    let directory = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(directory.path().join("repeat.sqlite"), "cwp://com.example.runtime", ConversationTimelineRollout::NewConversations).unwrap();
    let ConversationAdmissionOutcome::Started(turn) = store.admit(&graph, &ingress("repeat", person, None, ConversationIngressAddressing::Unaddressed, "Repeated"), None).unwrap() else { panic!("admission") };
    store.admit(&graph, &ingress("between-placements", person, Some(&turn.record.conversation_ref), ConversationIngressAddressing::Unaddressed, "Between placements"), None).unwrap();
    store.publish_timeline_message(&turn.record.conversation_ref, "cwp://com.example.runtime/publications/repeat", &turn.record.source_message_ref, ConversationTimelineDisposition::Root).unwrap();
    let revision = store.commit_revision;
    let error = store.fork_conversation(&graph, person, &turn.record.conversation_ref, &turn.record.source_message_ref, "ambiguous").unwrap_err();
    assert!(error.to_string().contains("repeated placements"));
    assert_eq!(store.commit_revision, revision);
    let timeline = store.retained_timeline(&turn.record.conversation_ref).unwrap();
    let first = timeline.connections[0].items[0].item_ref.clone();
    let last = timeline.connections[0].items.last().unwrap().item_ref.clone();
    let fork = store.fork_conversation_at(&graph, person, &turn.record.conversation_ref, &turn.record.source_message_ref, "explicit-placement", Some(&first)).unwrap();
    assert_eq!(store.retained_timeline(&fork.conversation_ref).unwrap().connections.iter().map(|c| c.items.len()).sum::<usize>(),1);
    let later = store.fork_conversation_at(&graph, person, &turn.record.conversation_ref, &turn.record.source_message_ref, "later-placement", Some(&last)).unwrap();
    assert_eq!(later.messages.len(), 2);
    assert_eq!(store.retained_timeline(&later.conversation_ref).unwrap().connections.iter().map(|c| c.items.len()).sum::<usize>(), 3);

}

#[test]
fn canonical_fork_keeps_published_admitted_work_without_updates() {
    let directory = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(directory.path().join("admitted-fork.sqlite"), "cwp://com.example.runtime", ConversationTimelineRollout::NewConversations).unwrap();
    let admitted = working_turn(&mut store, None, "not-started");
    let source = &admitted.record.conversation_ref;
    let original = admitted.record.invocation_ref.as_ref().unwrap();
    let ConversationAdmissionOutcome::Started(boundary) = store.admit(&graph, &ingress("after-admission",person,Some(source),ConversationIngressAddressing::Unaddressed,"Fork before work starts"),None).unwrap() else { panic!("boundary") };
    let timeline = store.retained_timeline(source).unwrap();
    let selected = timeline.connections.iter().flat_map(|c| &c.items).find(|item| item.content.reference() == boundary.record.source_message_ref).unwrap().item_ref.clone();
    let fork = store.fork_conversation_at(&graph,person,source,&boundary.record.source_message_ref,"admitted-history",Some(&selected)).unwrap();
    let copy = store.invocation_records().find(|r| r.conversation_ref.as_deref() == Some(&fork.conversation_ref)).unwrap();
    assert_eq!(copy.forked_from_invocation_ref.as_deref(),Some(original.as_str()));
    assert_eq!(copy.state,crate::v2026_09::invocation::ActionInvocationState::Stopped);
    assert!(!copy.is_runnable());
    assert!(store.retained_timeline(&fork.conversation_ref).unwrap().connections.iter().flat_map(|c| &c.items).any(|item| item.content.reference() == copy.invocation_ref));
    assert_eq!(store.invocation_record(original).unwrap().state,crate::v2026_09::invocation::ActionInvocationState::Admitted);
}

#[test]
fn canonical_fork_ignores_a_claim_and_completion_after_the_selected_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("late-start-fork.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(&path, root, ConversationTimelineRollout::NewConversations).unwrap();
    let turn = working_turn(&mut store, None, "late-start");
    let source = &turn.record.conversation_ref;
    let ConversationAdmissionOutcome::Started(boundary) = store.admit(&graph, &ingress("before-claim", person, Some(source), ConversationIngressAddressing::Unaddressed, "Before claim"), None).unwrap() else { panic!("boundary") };
    let claimed = claim_working(&mut store, &turn);
    store.fail(&claimed.adapter_invocation.invocation_ref, "Failure after boundary".into()).unwrap();
    let original = turn.record.invocation_ref.as_ref().unwrap();
    let fork = store.fork_conversation(&graph, person, source, &boundary.record.source_message_ref, "late-start-snapshot").unwrap();
    let copied = store.invocation_records().find(|r| r.conversation_ref.as_deref() == Some(&fork.conversation_ref)).unwrap().clone();
    assert_eq!(copied.started_at_unix_ms, None, "{:#?}", fork.activity);
    assert_eq!(copied.attempt_count, 0);
    assert_eq!(copied.state, crate::v2026_09::invocation::ActionInvocationState::Stopped);
    assert_eq!(copied.responses.len(), 1);
    assert!(copied.responses[0].result.is_none());
    assert!(!fork.messages.iter().any(|m| m.text == "Done"));
    assert!(!copied.is_runnable());
    assert!(!copied.is_live_root_completion());
    assert_eq!(store.invocation_record(original).unwrap().state, crate::v2026_09::invocation::ActionInvocationState::Failed);
    drop(store);
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(&path, root, ConversationTimelineRollout::NewConversations).unwrap();
    let retry = store.fork_conversation(&graph, person, source, &boundary.record.source_message_ref, "late-start-snapshot").unwrap();
    assert_eq!(retry, fork);
    assert_eq!(store.invocation_record(&copied.invocation_ref).unwrap(), &copied);
}

#[test]
fn canonical_fork_unique_selection_allows_other_repeated_content() {
    let directory = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(directory.path().join("unique-selection.sqlite"), "cwp://com.example.runtime", ConversationTimelineRollout::NewConversations).unwrap();
    let ConversationAdmissionOutcome::Started(first) = store.admit(&graph, &ingress("repeated-first", person, None, ConversationIngressAddressing::Unaddressed, "Repeated"), None).unwrap() else { panic!("first") };
    let source = &first.record.conversation_ref;
    store.publish_timeline_message(source, "cwp://com.example.runtime/publications/repeated-first", &first.record.source_message_ref, ConversationTimelineDisposition::Root).unwrap();
    let ConversationAdmissionOutcome::Started(selected) = store.admit(&graph, &ingress("unique-selection", person, Some(source), ConversationIngressAddressing::Unaddressed, "Unique"), None).unwrap() else { panic!("selected") };
    let fork = store.fork_conversation(&graph, person, source, &selected.record.source_message_ref, "unique-selection").unwrap();
    assert_eq!(fork.messages.len(), 2);
    assert_eq!(store.retained_timeline(&fork.conversation_ref).unwrap().connections.iter().map(|c| c.items.len()).sum::<usize>(), 3);
}

#[test]
fn canonical_fork_of_a_fork_preserves_frozen_completion() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fork-again.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(&path, root, ConversationTimelineRollout::NewConversations).unwrap();
    let turn = working_turn(&mut store, None, "complete-before-forks");
    let claimed = claim_working(&mut store, &turn);
    settle_working(&mut store, &claimed);
    let selected = store.conversation(&turn.record.conversation_ref).unwrap().messages.last().unwrap().message_ref.clone();
    let first = store.fork_conversation(&graph, person, &turn.record.conversation_ref, &selected, "first-generation").unwrap();
    let first_record = store.invocation_records().find(|r| r.conversation_ref.as_deref() == Some(&first.conversation_ref)).unwrap().clone();
    let selected = first.messages.last().unwrap().message_ref.clone();
    let second = store.fork_conversation(&graph, person, &first.conversation_ref, &selected, "second-generation").unwrap();
    let second_record = store.invocation_records().find(|r| r.conversation_ref.as_deref() == Some(&second.conversation_ref)).unwrap().clone();
    assert_eq!(second_record.state, first_record.state);
    assert_eq!(second_record.terminal_response().unwrap().summary, first_record.terminal_response().unwrap().summary);
    assert_eq!(second_record.started_at_unix_ms, first_record.started_at_unix_ms);
    assert_eq!(second_record.finished_at_unix_ms, first_record.finished_at_unix_ms);
    assert!(!second_record.is_runnable());
    assert!(!second_record.is_live_root_completion());
    drop(store);
    let store = ConversationAdmissionStore::open_with_timeline_rollout(&path, root, ConversationTimelineRollout::NewConversations).unwrap();
    assert_eq!(store.invocation_record(&second_record.invocation_ref).unwrap(), &second_record);
}

#[test]
fn canonical_fork_preserves_disclosed_inspector_with_hidden_source_across_generations() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("hidden-inspector.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "hidden-tool-source");
    let source = &turn.record.conversation_ref;
    let invocation = turn.record.invocation_ref.as_ref().unwrap();
    store.create_retained_timeline(source, &format!("{source}/timeline")).unwrap();
    store.retain_invocation_timeline_publication(invocation, &turn.record.source_message_ref, &format!("{source}/publications/work"), ConversationTimelineDisposition::Root).unwrap();
    store.configure_timeline_rollout(ConversationTimelineRollout::NewConversations).unwrap();
    claim_working(&mut store, &turn);
    inspector_tool(&mut store, invocation, "visible-command", false, "Visible before cutoff");
    let ConversationAdmissionOutcome::Started(boundary) = store.admit(&graph, &ingress("hidden-tool-boundary", person, Some(source), ConversationIngressAddressing::Unaddressed, "Visible boundary"), None).unwrap() else { panic!("boundary") };
    inspector_tool(&mut store, invocation, "visible-command", true, "After cutoff");
    let first = store.fork_conversation(&graph, person, source, &boundary.record.source_message_ref, "hidden-inspector-first").unwrap();
    let second = store.fork_conversation(&graph, person, &first.conversation_ref, &first.messages.last().unwrap().message_ref, "hidden-inspector-second").unwrap();
    for conversation in [&first, &second] {
        let copied = store.invocation_records().find(|r| r.conversation_ref.as_deref() == Some(&conversation.conversation_ref)).unwrap();
        assert!(copied.source_message_ref.is_none());
        let snapshot = inspector_snapshot(&store, person);
        let activities = snapshot.inspector_page(&copied.invocation_ref, ConversationInspectorCollection::Activities, &Default::default()).unwrap().unwrap();
        assert_eq!(activities.entries.len(), 1);
        let output = snapshot.inspector_text(&copied.invocation_ref, &activities.entries[0].0.reference, None, ConversationInspectorTextField::Output).unwrap().unwrap();
        assert_eq!(output.text, "Visible before cutoff");
    }
    drop(store);
    ConversationAdmissionStore::open_with_timeline_rollout(&path, root, ConversationTimelineRollout::NewConversations).unwrap();
}

#[test]
fn canonical_fork_earlier_placement_keeps_an_inherited_work_snapshot_immutable() {
    let directory = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(directory.path().join("immutable-snapshot.sqlite"), "cwp://com.example.runtime", ConversationTimelineRollout::NewConversations).unwrap();
    let turn = working_turn(&mut store, None, "snapshot-before-refork");
    let claimed = claim_working(&mut store, &turn);
    let invocation = turn.record.invocation_ref.as_ref().unwrap();
    inspector_tool(&mut store, invocation, "snapshot-command", false, "Before progress");
    store.record_invocation_progress_with_publication(invocation, 102, "Earlier progress", true).unwrap();
    inspector_tool(&mut store, invocation, "snapshot-command", true, "Frozen completed output");
    settle_working(&mut store, &claimed);
    let selected = store.conversation(&turn.record.conversation_ref).unwrap().messages.last().unwrap().message_ref.clone();
    let first = store.fork_conversation(&graph, person, &turn.record.conversation_ref, &selected, "immutable-first").unwrap();
    let earlier = first.messages.iter().find(|m| m.text == "Earlier progress").unwrap().message_ref.clone();
    let second = store.fork_conversation(&graph, person, &first.conversation_ref, &earlier, "immutable-earlier").unwrap();
    assert!(!second.messages.iter().any(|m| m.text == "Done"));
    let copied = store.invocation_records().find(|r| r.conversation_ref.as_deref() == Some(&second.conversation_ref)).unwrap();
    assert_eq!(copied.state, crate::v2026_09::invocation::ActionInvocationState::Completed);
    assert!(copied.reply_message_ref.is_none());
    let snapshot = inspector_snapshot(&store, person);
    let activities = snapshot.inspector_page(&copied.invocation_ref, ConversationInspectorCollection::Activities, &Default::default()).unwrap().unwrap();
    assert_eq!(activities.entries.len(), 1);
    let output = snapshot.inspector_text(&copied.invocation_ref, &activities.entries[0].0.reference, None, ConversationInspectorTextField::Output).unwrap().unwrap();
    assert_eq!(output.text, "Frozen completed output");
}

#[test]
fn canonical_fork_cutoff_includes_every_publication_in_the_visual_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(directory.path().join("prefix-publications.sqlite"), "cwp://com.example.runtime", ConversationTimelineRollout::NewConversations).unwrap();
    let turn = working_turn(&mut store, None, "repeated-work-source");
    claim_working(&mut store, &turn);
    let source = &turn.record.conversation_ref;
    let invocation = turn.record.invocation_ref.as_ref().unwrap();
    inspector_tool(&mut store, invocation, "republished-command", false, "Before selected message");
    let ConversationAdmissionOutcome::Started(boundary) = store.admit(&graph, &ingress("before-republication", person, Some(source), ConversationIngressAddressing::Unaddressed, "Selected boundary"), None).unwrap() else { panic!("boundary") };
    inspector_tool(&mut store, invocation, "republished-command", true, "Before the new visible publication");
    let parent = store.retained_timeline(source).unwrap().connections.iter().flat_map(|c| &c.items).find(|item| item.content.reference() == invocation).unwrap().item_ref.clone();
    // Manual timeline APIs support repeated placements; managed writers reject them.
    store.timeline_rollout = None;
    store.retain_invocation_timeline_publication(invocation, &turn.record.source_message_ref, "cwp://com.example.runtime/publications/repeated-prefix-work", ConversationTimelineDisposition::Child { parent_item_ref: parent }).unwrap();
    let fork = store.fork_conversation(&graph, person, source, &boundary.record.source_message_ref, "prefix-publications").unwrap();
    let copied = store.invocation_records().find(|r| r.conversation_ref.as_deref() == Some(&fork.conversation_ref)).unwrap();
    let snapshot = inspector_snapshot(&store, person);
    let activities = snapshot.inspector_page(&copied.invocation_ref, ConversationInspectorCollection::Activities, &Default::default()).unwrap().unwrap();
    assert_eq!(activities.entries.len(), 1);
    let output = snapshot.inspector_text(&copied.invocation_ref, &activities.entries[0].0.reference, None, ConversationInspectorTextField::Output).unwrap().unwrap();
    assert_eq!(output.text, "Before the new visible publication");
}

#[test]
fn canonical_fork_id_binds_one_placement_across_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fork-id-binding.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(&path, root, ConversationTimelineRollout::NewConversations).unwrap();
    let ConversationAdmissionOutcome::Started(turn) = store.admit(&graph, &ingress("fork-id-boundary", person, None, ConversationIngressAddressing::Unaddressed, "Repeated boundary"), None).unwrap() else { panic!("boundary") };
    let source = &turn.record.conversation_ref;
    let message = &turn.record.source_message_ref;
    store.publish_timeline_message(source, "cwp://com.example.runtime/publications/fork-id-repeat", message, ConversationTimelineDisposition::Root).unwrap();
    let timeline = store.retained_timeline(source).unwrap();
    let placements = timeline.connections.iter().flat_map(|c| &c.items).filter(|item| item.content.reference() == message).map(|item| item.item_ref.clone()).collect::<Vec<_>>();
    assert_eq!(placements.len(), 2);
    let fork = store.fork_conversation_at(&graph, person, source, message, "one-operation", Some(&placements[0])).unwrap();
    assert_eq!(store.fork_conversation_at(&graph, person, source, message, "one-operation", Some(&placements[0])).unwrap(), fork);
    let revision = store.commit_revision;
    assert!(store.fork_conversation_at(&graph, person, source, message, "one-operation", Some(&placements[1])).unwrap_err().to_string().contains("different arguments"));
    assert_eq!(store.commit_revision, revision);
    drop(store);
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(&path, root, ConversationTimelineRollout::NewConversations).unwrap();
    assert_eq!(store.fork_conversation_at(&graph, person, source, message, "one-operation", Some(&placements[0])).unwrap(), fork);
    let revision = store.commit_revision;
    assert!(store.fork_conversation_at(&graph, person, source, message, "one-operation", Some(&placements[1])).unwrap_err().to_string().contains("different arguments"));
    assert_eq!(store.commit_revision, revision);
}
