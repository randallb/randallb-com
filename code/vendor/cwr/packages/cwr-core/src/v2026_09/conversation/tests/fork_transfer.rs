#[test]
fn conversation_fork_captures_the_selected_message_boundary_and_is_idempotent() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("fork.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let (source_ref, invocation_ref, selected_ref, later_ref) =
        create_fork_source(&mut store, &graph, person);
    store
        .record_measured_invocation_progress(
            &invocation_ref,
            102,
            "Later keyed progress",
            true,
            Some(crate::v2026_09::invocation::ActionProgress {
                key: "phase".into(),
                phase: Some("after".into()),
                completed: None,
                total: None,
                unit: None,
            }),
        )
        .unwrap();
    store
        .complete(&invocation_ref, json!({"text": "Later result"}))
        .unwrap();

    let original = store.conversation(&source_ref).unwrap().clone();
    let source_invocation_count = store.invocation_records().count();
    let progress_message_ref = original
        .messages
        .iter()
        .find(|message| message.text == "Working before the fork point")
        .unwrap()
        .message_ref
        .clone();
    let progress_fork = store
        .fork_conversation(
            &graph,
            person,
            &source_ref,
            &progress_message_ref,
            "progress-operation",
        )
        .unwrap();
    assert!(progress_fork.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::WorkUpdate { status, text, .. }
            if *status == ActionResponseStatus::PROGRESS
                && text.as_deref() == Some("Working before the fork point")
    )));
    assert!(!progress_fork.activity.iter().any(|item| {
        serde_json::to_string(item)
            .unwrap()
            .contains("Later keyed progress")
    }));
    assert_has_stopped_work(&progress_fork);
    let terminal_message_ref = original
        .messages
        .iter()
        .find(|message| message.text == "Later result")
        .unwrap()
        .message_ref
        .clone();
    let terminal_fork = store
        .fork_conversation(
            &graph,
            person,
            &source_ref,
            &terminal_message_ref,
            "terminal-operation",
        )
        .unwrap();
    assert_has_completed_work(&terminal_fork);
    let copied_terminal_message_ref = terminal_fork
        .messages
        .iter()
        .find(|message| message.text == "Later result")
        .unwrap()
        .message_ref
        .clone();
    let repeated_terminal_fork = store
        .fork_conversation(
            &graph,
            person,
            &terminal_fork.conversation_ref,
            &copied_terminal_message_ref,
            "repeated-terminal-operation",
        )
        .unwrap();
    assert_has_completed_work(&repeated_terminal_fork);
    assert!(repeated_terminal_fork.activity.iter().any(|item| matches!(
        item.event,
        ConversationActivityEvent::ConversationForkedFrom {
            lineage_depth: 2,
            ..
        }
    )));
    assert_eq!(lineage_count(&repeated_terminal_fork), 2);
    assert_imported_terminal_status_wins_over_later_progress(
        &mut store,
        &graph,
        person,
        &source_ref,
    );
    let fork = store
        .fork_conversation(
            &graph,
            person,
            &source_ref,
            &selected_ref,
            "stable-operation",
        )
        .unwrap();
    assert_eq!(
        fork.messages
            .iter()
            .filter(|message| message.progress.is_none())
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        vec!["First", "Selected"]
    );
    assert!(!fork
        .activity
        .iter()
        .any(|item| serde_json::to_string(item).unwrap().contains("Later result")));
    assert!(!fork.activity.iter().any(|item| {
        serde_json::to_string(item)
            .unwrap()
            .contains("Later keyed progress")
    }));
    assert!(fork.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::WorkUpdate { status, text, .. }
            if *status == ActionResponseStatus::PROGRESS
                && text.as_deref() == Some("Working before the fork point")
    )));
    assert_metadata_lineage_visibility(&store, &fork, &source_ref);
    assert_eq!(store.invocation_records().count(), source_invocation_count);
    assert!(store
        .invocation_records()
        .all(|record| record.conversation_ref.as_deref() == Some(&source_ref)));
    assert!(fork.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::ConversationForkedFrom {
            source_conversation_ref,
            source_message_ref,
            operation_id,
            ..
        } if source_conversation_ref == &source_ref
            && source_message_ref == &selected_ref
            && operation_id == "stable-operation"
    )));
    assert_eq!(store.conversation(&source_ref), Some(&original));
    assert_eq!(
        store
            .fork_conversation(
                &graph,
                person,
                &source_ref,
                &selected_ref,
                "stable-operation"
            )
            .unwrap(),
        fork,
    );
    assert!(store
        .fork_conversation(
            &graph,
            person,
            &source_ref,
            &later_ref,
            "stable-operation"
        )
        .is_err());
    assert!(store
        .fork_conversation(
            &graph,
            "stranger",
            &source_ref,
            &selected_ref,
            "unauthorized"
        )
        .is_err());

    drop(store);
    assert_backward_readable_storage(&path);
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    assert_eq!(
        store
            .fork_conversation(
                &graph,
                person,
                &source_ref,
                &selected_ref,
                "stable-operation"
            )
            .unwrap(),
        fork,
    );
    assert_metadata_lineage_visibility(&store, &fork, &source_ref);
}

#[test]
fn export_resolves_each_reply_from_the_indexed_execution_conversation() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("indexed-execution-replies.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = admit_context_turn(&mut store, &graph, None, "first-indexed", "First request");
    let conversation_ref = first.conversation.conversation_ref;
    let first_invocation_ref = first.record.invocation_ref.as_deref().unwrap().to_owned();
    assert!(matches!(
        store.claim_invocation(&first_invocation_ref, 100).unwrap(),
        ConversationInvocationClaimOutcome::Claimed(_)
    ));
    store
        .complete(&first_invocation_ref, json!({"text": "First result"}))
        .unwrap();
    let second = admit_context_turn(
        &mut store,
        &graph,
        Some(&conversation_ref),
        "second-indexed",
        "Second request",
    );
    let second_invocation_ref = second.record.invocation_ref.as_deref().unwrap().to_owned();
    assert!(matches!(
        store.claim_invocation(&second_invocation_ref, 101).unwrap(),
        ConversationInvocationClaimOutcome::Claimed(_)
    ));
    store
        .complete(&second_invocation_ref, json!({"text": "Second result"}))
        .unwrap();

    let export = store.export_conversation(&conversation_ref, person).unwrap();
    let terminal_by_ref = export
        .conversation
        .activity
        .iter()
        .filter_map(|item| match &item.event {
            ConversationActivityEvent::WorkUpdate {
                invocation_ref,
                status,
                ..
            } if status.is_terminal() => Some((item.activity_ref.as_str(), invocation_ref.as_str())),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let associated_invocations = export
        .conversation
        .activity
        .iter()
        .filter_map(|item| match &item.event {
            ConversationActivityEvent::MessageWorkAssociation { activity_ref, .. } => {
                terminal_by_ref.get(activity_ref.as_str()).copied()
            }
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        associated_invocations,
        std::collections::BTreeSet::from([
            first_invocation_ref.as_str(),
            second_invocation_ref.as_str(),
        ])
    );
}

fn assert_imported_terminal_status_wins_over_later_progress(
    store: &mut ConversationAdmissionStore,
    graph: &ResolvedCwrGraph,
    person: &str,
    source_ref: &str,
) {
    let mut ordinary_export = v1_export(store.export_conversation(source_ref, person).unwrap());
    let progress_index = ordinary_export
        .conversation
        .activity
        .iter()
        .position(|item| matches!(
            item.event,
            ConversationActivityEvent::WorkUpdate {
                status: ActionResponseStatus::PROGRESS,
                ..
            }
        ))
        .unwrap();
    let terminal_index = ordinary_export
        .conversation
        .activity
        .iter()
        .position(|item| matches!(
            item.event,
            ConversationActivityEvent::WorkUpdate { status, .. }
                if status.is_terminal()
        ))
        .unwrap();
    ordinary_export
        .conversation
        .activity
        .swap(progress_index, terminal_index);
    let ordinary_import = store
        .import_conversation(graph, person, ordinary_export, None)
        .unwrap();
    let imported_terminal_message_ref = ordinary_import
        .messages
        .iter()
        .find(|message| message.text == "Later result")
        .unwrap()
        .message_ref
        .clone();
    let imported_terminal_fork = store
        .fork_conversation(
            graph,
            person,
            &ordinary_import.conversation_ref,
            &imported_terminal_message_ref,
            "imported-terminal-operation",
        )
        .unwrap();
    assert_has_completed_work(&imported_terminal_fork);
    assert!(imported_terminal_fork.activity.iter().any(|item| matches!(
        item.event,
        ConversationActivityEvent::WorkLifecycle {
            state: crate::v2026_09::invocation::ActionInvocationState::Completed,
            ..
        }
    )));
    assert!(!imported_terminal_fork.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::WorkUpdate { text: Some(text), .. }
            if text == "Working before the fork point"
    )));
}

#[test]
fn fork_and_import_ids_have_separate_destinations() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("fork-import-ids.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let ConversationAdmissionOutcome::Started(source) = store
        .admit(
            &graph,
            &ingress(
                "fork-import-source",
                person,
                None,
                ConversationIngressAddressing::Unaddressed,
                "Source",
            ),
            None,
        )
        .unwrap()
    else {
        panic!("source conversation")
    };
    let source_ref = source.conversation.conversation_ref;
    let message_ref = source.conversation.messages[0].message_ref.clone();
    let export = store.export_conversation(&source_ref, person).unwrap();
    let imported_first = store
        .import_conversation(&graph, person, export.clone(), Some("fork:first"))
        .unwrap();
    let forked_second = store
        .fork_conversation(&graph, person, &source_ref, &message_ref, "first")
        .unwrap();
    assert_ne!(imported_first.conversation_ref, forked_second.conversation_ref);

    let forked_first = store
        .fork_conversation(&graph, person, &source_ref, &message_ref, "second")
        .unwrap();
    let imported_second = store
        .import_conversation(&graph, person, export, Some("fork:second"))
        .unwrap();
    assert_ne!(forked_first.conversation_ref, imported_second.conversation_ref);
}

fn assert_metadata_lineage_visibility(
    store: &ConversationAdmissionStore,
    fork: &RetainedConversation,
    source_ref: &str,
) {
    let metadata = crate::v2026_09::host::project_host_conversation_metadata(
        store,
        &fork.conversation_ref,
    )
    .unwrap()
    .unwrap();
    assert!(metadata.activity.iter().any(|item| matches!(
        item.event,
        ConversationActivityEvent::ConversationForkedFrom { .. }
    )));
    assert!(crate::v2026_09::host::project_full_conversation_metadata(fork)
        .activity
        .iter()
        .any(|item| matches!(
            &item.event,
            ConversationActivityEvent::ConversationForkedFrom {
                source_conversation_ref,
                ..
            } if source_conversation_ref == source_ref
        )));
    let bootstrap = crate::v2026_09::host::project_host_conversation_bootstrap(store, true).unwrap();
    assert!(bootstrap
        .iter()
        .find(|conversation| conversation.conversation_ref == fork.conversation_ref)
        .unwrap()
        .activity
        .iter()
        .any(|item| matches!(
            item.event,
            ConversationActivityEvent::ConversationForkedFrom { .. }
        )));
}

#[test]
fn nested_reply_fork_retains_ancestors_only_through_the_selected_boundary() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let reviewer = graph
        .actors
        .iter()
        .find(|actor| actor.coworker_slug == "reviewer")
        .unwrap()
        .actor_ref
        .clone();
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("nested-fork.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let ConversationAdmissionOutcome::Started(started) = store
        .admit(
            &graph,
            &ingress(
                "nested-source",
                person,
                None,
                ConversationIngressAddressing::Unaddressed,
                "Start nested work",
            ),
            None,
        )
        .unwrap()
    else {
        panic!("new conversation")
    };
    let mut export = store
        .export_conversation(&started.conversation.conversation_ref, person)
        .unwrap();
    let source_message_ref = export.conversation.messages[0].message_ref.clone();
    let mut parent_progress = export.conversation.messages[0].clone();
    parent_progress.message_ref = format!("{source_message_ref}/parent-progress");
    parent_progress.reply_to_message_ref = Some(source_message_ref.clone());
    parent_progress.author_actor_ref = reviewer.clone();
    parent_progress.text = "Parent before child".into();
    parent_progress.progress = Some(Box::new(ConversationProgress {
        invocation_ref: "invocation/parent".into(),
        source_activity_ref: "event/parent-before".into(),
        status: Some(ActionResponseStatus::PROGRESS),
    }));
    export.conversation.record_message(parent_progress.clone());
    let mut reply = export.conversation.messages[0].clone();
    reply.message_ref = format!("{source_message_ref}/nested-reply");
    reply.reply_to_message_ref = Some(source_message_ref.clone());
    reply.author_actor_ref = reviewer.clone();
    reply.text = "Nested result".into();
    export.conversation.record_message(reply.clone());
    for (activity_ref, invocation_ref, parent_invocation_ref, status, text) in [
        (
            "event/parent-before",
            "invocation/parent",
            None,
            ActionResponseStatus::PROGRESS,
            "Parent before child",
        ),
        (
            "event/parent-during",
            "invocation/parent",
            None,
            ActionResponseStatus::PROGRESS,
            "Parent during child",
        ),
        (
            "event/child-terminal",
            "invocation/child",
            Some("invocation/parent"),
            ActionResponseStatus::COMPLETED,
            "Nested result",
        ),
        (
            "event/parent-after",
            "invocation/parent",
            None,
            ActionResponseStatus::PROGRESS,
            "Parent after child",
        ),
    ] {
        export.conversation.activity.push(ConversationActivity {
            activity_ref: activity_ref.into(),
            sequence: export.conversation.activity.len() as u64 + 1,
            occurred_at_unix_ms: export.exported_at_unix_ms,
            event: ConversationActivityEvent::WorkUpdate {
                invocation_ref: invocation_ref.into(),
                source_message_ref: source_message_ref.clone(),
                parent_invocation_ref: parent_invocation_ref.map(str::to_owned),
                wait: None,
                actor_ref: reviewer.clone(),
                status,
                text: Some(text.into()),
                text_kind: Some(WorkUpdateTextKind::Commentary),
                activity: None,
            },
        });
    }
    export.conversation.activity.push(ConversationActivity {
        activity_ref: "event/parent-association".into(),
        sequence: export.conversation.activity.len() as u64 + 1,
        occurred_at_unix_ms: export.exported_at_unix_ms,
        event: ConversationActivityEvent::MessageWorkAssociation {
            message_ref: parent_progress.message_ref,
            activity_ref: "event/parent-before".into(),
        },
    });
    export.conversation.activity.push(ConversationActivity {
        activity_ref: "event/nested-association".into(),
        sequence: export.conversation.activity.len() as u64 + 1,
        occurred_at_unix_ms: export.exported_at_unix_ms,
        event: ConversationActivityEvent::MessageWorkAssociation {
            message_ref: reply.message_ref.clone(),
            activity_ref: "event/child-terminal".into(),
        },
    });
    let imported = store
        .import_conversation(&graph, person, export, None)
        .unwrap();
    let selected = imported
        .messages
        .iter()
        .find(|message| message.text == "Nested result")
        .unwrap()
        .message_ref
        .clone();
    let fork = store
        .fork_conversation(
            &graph,
            person,
            &imported.conversation_ref,
            &selected,
            "nested-reply-boundary",
        )
        .unwrap();
    let retained_text = fork
        .activity
        .iter()
        .filter_map(|item| match &item.event {
            ConversationActivityEvent::WorkUpdate { text, .. } => text.as_deref(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(retained_text.contains(&"Parent before child"));
    assert!(retained_text.contains(&"Parent during child"));
    assert!(retained_text.contains(&"Nested result"));
    assert!(!retained_text.contains(&"Parent after child"));
    assert!(fork.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::WorkUpdate {
            parent_invocation_ref: Some(_),
            text: Some(text),
            ..
        } if text == "Nested result"
    )));
}
