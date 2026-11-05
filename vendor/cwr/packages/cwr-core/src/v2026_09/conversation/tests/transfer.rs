#[test]
fn conversation_transfer_forks_public_history_and_survives_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let ConversationAdmissionOutcome::Started(started) = store
        .admit(
            &graph,
            &ingress(
                "transfer-source",
                person,
                None,
                ConversationIngressAddressing::Unaddressed,
                "A long message\nwith formatting and 🦀",
            ),
            None,
        )
        .unwrap()
    else {
        panic!("new conversation")
    };
    let reference = started.conversation.conversation_ref;
    store
        .rename_conversation(&reference, person, "Fork me")
        .unwrap();
    store
        .set_conversation_archived(&reference, person, true)
        .unwrap();
    let source_message = store.conversation(&reference).unwrap().messages[0]
        .message_ref
        .clone();
    store
        .transact(|state| {
            let conversation = state
                .conversations
                .find_mut(|c| c.conversation_ref == reference)
                .unwrap();
            conversation.activity.push(ConversationActivity {
                activity_ref: "cwp://example.test/work/lifecycle".into(),
                sequence: 2,
                occurred_at_unix_ms: 1,
                event: ConversationActivityEvent::WorkLifecycle {
                    invocation_ref: "cwp://example.test/invocations/old".into(),
                    actor_ref: person.into(),
                    source_message_ref: source_message.clone(),
                    parent_invocation_ref: None,
                    awaited_by_invocation_ref: None,
                    state: crate::v2026_09::invocation::ActionInvocationState::Running,
                    admitted_at_unix_ms: 1,
                    started_at_unix_ms: Some(1),
                    stop_requested_at_unix_ms: None,
                    finished_at_unix_ms: None,
                },
            });
            conversation.activity.push(ConversationActivity {
                activity_ref: "cwp://example.test/provider".into(),
                sequence: 3,
                occurred_at_unix_ms: 2,
                event: ConversationActivityEvent::ProviderThread {
                    provider: "private".into(),
                    thread_id: "do-not-export".into(),
                },
            });
            Ok(())
        })
        .unwrap();
    let original = store.conversation(&reference).unwrap().clone();
    let export = store.export_conversation(&reference, person).unwrap();
    let serialized = serde_json::to_value(&export).unwrap();
    assert!(!serialized.to_string().contains("do-not-export"));
    assert!(serialized["conversation"].get("archived").is_none());
    assert!(serialized["conversation"].get("sidebar_eligible").is_none());
    let exported_v1: ConversationExport = serde_json::from_value(serialized).unwrap();
    assert!(store.export_conversation(&reference, "stranger").is_err());
    let fork = store
        .import_conversation(&graph, person, exported_v1.clone(), Some("import-attempt"))
        .unwrap();
    assert_eq!(
        store
            .import_conversation(&graph, person, exported_v1.clone(), Some("import-attempt"))
            .unwrap(),
        fork
    );
    let second = store
        .import_conversation(&graph, person, exported_v1, None)
        .unwrap();
    assert_ne!(second.conversation_ref, fork.conversation_ref);
    assert_ne!(fork.conversation_ref, reference);
    assert_eq!(fork.title.as_deref(), Some("Fork me"));
    assert!(!fork.archived);
    assert_eq!(fork.sidebar_eligible, Some(true));
    assert!(fork.is_sidebar_eligible());
    assert_eq!(fork.messages[0].text, original.messages[0].text);
    assert_eq!(
        fork.messages[0].author_actor_ref,
        original.messages[0].author_actor_ref
    );
    assert_ne!(fork.messages[0].message_ref, source_message);
    assert!(fork.activity.iter().any(|item| matches!(
        item.event,
        ConversationActivityEvent::WorkLifecycle {
            state: crate::v2026_09::invocation::ActionInvocationState::Stopped,
            ..
        }
    )));
    assert_eq!(store.invocation_records().count(), 0);
    assert_eq!(store.conversation(&reference), Some(&original));
    drop(store);
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    assert_eq!(store.conversation(&fork.conversation_ref), Some(&fork));
    store
        .admit(
            &graph,
            &ingress(
                "continue-fork",
                person,
                Some(&fork.conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "Continue from here",
            ),
            None,
        )
        .unwrap();
    assert_eq!(
        store
            .conversation(&fork.conversation_ref)
            .unwrap()
            .messages
            .len(),
        2
    );
    assert_eq!(store.conversation(&reference), Some(&original));
    let next = admit_context_turn(
        &mut store,
        &graph,
        Some(&fork.conversation_ref),
        "ask-helper-in-fork",
        "Use the imported history",
    );
    let invocation_ref = next.record.invocation_ref.as_deref().unwrap();
    let ConversationInvocationClaimOutcome::Claimed(claimed) =
        store.claim_invocation(invocation_ref, 100).unwrap()
    else {
        panic!("new coworker invocation")
    };
    assert!(claimed.adapter_invocation.continuation.is_none());
    let history = claimed.adapter_invocation.thread["messages"]
        .as_array()
        .unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0]["text"], original.messages[0].text);
    assert_eq!(history[2]["text"], "Use the imported history");
}

fn create_fork_source(
    store: &mut ConversationAdmissionStore,
    graph: &crate::v2026_09::graph::ResolvedCwrGraph,
    person: &str,
) -> (String, String, String, String) {
    let started = admit_context_turn(store, graph, None, "fork-source", "First");
    let source_ref = started.conversation.conversation_ref;
    let invocation_ref = started.record.invocation_ref.as_deref().unwrap().to_owned();
    let ConversationInvocationClaimOutcome::Claimed(_) =
        store.claim_invocation(&invocation_ref, 100).unwrap()
    else {
        panic!("claim parallel work")
    };
    store
        .record_measured_invocation_progress(
            &invocation_ref,
            102,
            "Working before the fork point",
            true,
            Some(crate::v2026_09::invocation::ActionProgress {
                key: "phase".into(),
                phase: Some("before".into()),
                completed: None,
                total: None,
                unit: None,
            }),
        )
        .unwrap();
    store
        .admit(
            graph,
            &ingress(
                "fork-selected",
                person,
                Some(&source_ref),
                ConversationIngressAddressing::Unaddressed,
                "Selected",
            ),
            None,
        )
        .unwrap();
    let selected_ref = store
        .conversation(&source_ref)
        .unwrap()
        .messages
        .iter()
        .find(|message| message.text == "Selected")
        .unwrap()
        .message_ref
        .clone();
    store
        .admit(
            graph,
            &ingress(
                "fork-later",
                person,
                Some(&source_ref),
                ConversationIngressAddressing::Unaddressed,
                "Later",
            ),
            None,
        )
        .unwrap();
    let later_ref = store
        .conversation(&source_ref)
        .unwrap()
        .messages
        .iter()
        .find(|message| message.text == "Later")
        .unwrap()
        .message_ref
        .clone();
    (source_ref, invocation_ref, selected_ref, later_ref)
}

#[test]
fn conversation_fork_rejects_action_call_boundaries() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("action-call-fork.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let started = admit_context_turn(&mut store, &graph, None, "action-call-source", "First");
    let conversation_ref = started.conversation.conversation_ref;
    let message_ref = store
        .append_action_call(
            &conversation_ref,
            person,
            "cwp://com.example.fixture/actors/reviewer",
            "@reviewer--check",
            json!({}),
            "Reviewing".into(),
        )
        .unwrap()
        .message_ref;

    let error = store
        .fork_conversation(
            &graph,
            person,
            &conversation_ref,
            &message_ref,
            "action-call-operation",
        )
        .unwrap_err();
    assert!(error.to_string().contains("action-call message"));
}

fn assert_has_completed_work(conversation: &RetainedConversation) {
    assert!(conversation.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::WorkUpdate { status, .. }
            if *status == ActionResponseStatus::COMPLETED
    )));
}

fn assert_has_stopped_work(conversation: &RetainedConversation) {
    assert!(conversation.activity.iter().any(|item| matches!(
        item.event,
        ConversationActivityEvent::WorkLifecycle {
            state: crate::v2026_09::invocation::ActionInvocationState::Stopped,
            ..
        }
    )));
}

fn lineage_count(conversation: &RetainedConversation) -> usize {
    conversation
        .activity
        .iter()
        .filter(|item| {
            matches!(
                item.event,
                ConversationActivityEvent::ConversationForkedFrom { .. }
            )
        })
        .count()
}

#[test]
fn imported_conversation_inherits_the_last_valid_addressed_route() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let helper_integration = graph
        .actors
        .iter()
        .find(|actor| actor.actor_ref == helper)
        .and_then(|actor| actor.default_actor_integration_ref.as_deref())
        .unwrap();
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("route.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let ConversationAdmissionOutcome::Started(started) = store
        .admit(
            &graph,
            &ingress(
                "addressed-source",
                person,
                None,
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![helper.into()],
                },
                "Keep helping after import",
            ),
            Some(helper_integration),
        )
        .unwrap()
    else {
        panic!("addressed source")
    };
    let export = store
        .export_conversation(&started.conversation.conversation_ref, person)
        .unwrap();
    let fork = store
        .import_conversation(&graph, person, export, None)
        .unwrap();

    let ConversationAdmissionOutcome::Started(continued) = store
        .admit(
            &graph,
            &ingress(
                "implicit-import-continuation",
                person,
                Some(&fork.conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "Continue with the imported context",
            ),
            None,
        )
        .unwrap()
    else {
        panic!("continued import")
    };

    assert_eq!(continued.record.target_actor_ref.as_deref(), Some(helper));
    assert!(continued.record.invocation_ref.is_some());
}

#[test]
fn ordinary_host_exchanges_do_not_create_inherited_routes() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let host = "cwp://org.coworkerprotocol.local/integrations/desktop";
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("host-route.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let exchange = store
        .record_host_exchange(
            None,
            person,
            host,
            "Open settings".into(),
            "Settings opened".into(),
        )
        .unwrap();

    let continued = store
        .admit(
            &graph,
            &ingress(
                "host-continuation",
                person,
                Some(&exchange.conversation.conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "Continue here",
            ),
            None,
        )
        .unwrap();

    let ConversationAdmissionOutcome::Started(continued) = continued else {
        panic!("continued host exchange")
    };
    assert!(continued.record.target_actor_ref.is_none());
    assert!(continued.record.invocation_ref.is_none());
}

#[test]
fn conversation_transfer_preserves_reply_and_nested_work_links() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("links.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let ConversationAdmissionOutcome::Started(started) = store
        .admit(
            &graph,
            &ingress(
                "linked-export",
                person,
                None,
                ConversationIngressAddressing::Unaddressed,
                "Original",
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
    let mut reply = export.conversation.messages[0].clone();
    reply.reply_to_message_ref = Some(reply.message_ref.clone());
    reply.message_ref.push_str("/reply");
    reply.text = "Reply\nwith **formatting**".into();
    export.conversation.record_message(reply);
    let message = export.conversation.messages[0].message_ref.clone();
    for (invocation, parent, status, child) in [
        (
            "parent",
            None,
            ActionResponseStatus::COMPLETED,
            Some("child"),
        ),
        (
            "child",
            Some("parent"),
            ActionResponseStatus::PROGRESS,
            None,
        ),
    ] {
        export.conversation.activity.push(ConversationActivity {
            activity_ref: format!("event/{invocation}"),
            sequence: export.conversation.activity.len() as u64 + 1,
            occurred_at_unix_ms: export.exported_at_unix_ms,
            event: ConversationActivityEvent::WorkUpdate {
                invocation_ref: invocation.into(),
                source_message_ref: message.clone(),
                parent_invocation_ref: parent.map(str::to_owned),
                wait: None,
                actor_ref: person.into(),
                status,
                text: Some("Keep this work text".into()),
                text_kind: Some(WorkUpdateTextKind::Commentary),
                activity: Some(Box::new(WorkActivity {
                    activity_ref: format!("tool/{invocation}"),
                    kind: "command".into(),
                    label: "Inspect files".into(),
                    state: "completed".into(),
                    started_at_unix_ms: 1,
                    child_invocation_ref: child.map(str::to_owned),
                    duration_ms: Some(1),
                    command: Some("ls".into()),
                    output: Some(message.clone()),
                    error: None,
                    exit_code: Some(0),
                    files: Vec::new(),
                })),
            },
        });
    }
    export.conversation.activity.push(ConversationActivity {
        activity_ref: "event/reply-association".into(),
        sequence: export.conversation.activity.len() as u64 + 1,
        occurred_at_unix_ms: export.exported_at_unix_ms,
        event: ConversationActivityEvent::MessageWorkAssociation {
            message_ref: export.conversation.messages[1].message_ref.clone(),
            activity_ref: "event/parent".into(),
        },
    });
    let fork = store
        .import_conversation(&graph, person, export, None)
        .unwrap();
    assert_eq!(
        fork.messages[1].reply_to_message_ref.as_ref(),
        Some(&fork.messages[0].message_ref)
    );
    assert_eq!(fork.messages[1].text, "Reply\nwith **formatting**");
    let updates = fork
        .activity
        .iter()
        .filter_map(|item| match &item.event {
            ConversationActivityEvent::WorkUpdate {
                invocation_ref,
                parent_invocation_ref,
                activity,
                status,
                ..
            } => Some((invocation_ref, parent_invocation_ref, activity, status)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(updates.len(), 3);
    assert_eq!(updates[1].1.as_ref(), Some(updates[0].0));
    assert_eq!(
        updates[0].2.as_ref().unwrap().child_invocation_ref.as_ref(),
        Some(updates[1].0)
    );
    assert_eq!(
        updates[0].2.as_ref().unwrap().output.as_ref(),
        Some(&message)
    );
    assert_eq!(*updates[2].3, ActionResponseStatus::STOPPED);
    assert_eq!(store.invocation_records().count(), 0);
}

#[test]
fn imported_completions_have_destination_order_before_new_work_and_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("import-order.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let (source, invocation, _, _) = create_fork_source(&mut store, &graph, person);
    store
        .complete(&invocation, json!({"text":"Result"}))
        .unwrap();
    let export = store.export_conversation(&source, person).unwrap();
    let imported = store
        .import_conversation(&graph, person, export, None)
        .unwrap();
    let read = |store: &ConversationAdmissionStore| {
        let mut reader = store.read_store().unwrap();
        let result = reader
            .begin_operation(std::time::Duration::from_secs(2))
            .unwrap()
            .root_conversations(ConversationSummaryWindow {
                first: Some(100),
                ..Default::default()
            })
            .unwrap()
            .summaries
            .into_iter()
            .find(|row| row.key.conversation_ref == imported.conversation_ref)
            .unwrap();
        result
    };
    let before = read(&store);
    assert!(!before.completed_work_refs.is_empty());
    assert_eq!(before.completed_work.len(), 1);
    let max = before
        .completed_work
        .iter()
        .map(|item| item.sequence)
        .max()
        .unwrap();
    let next = admit_context_turn(
        &mut store,
        &graph,
        Some(&imported.conversation_ref),
        "after-import",
        "Continue",
    );
    let invocation = next.record.invocation_ref.as_ref().unwrap();
    let ConversationInvocationClaimOutcome::Claimed(_) =
        store.claim_invocation(invocation, 1).unwrap()
    else {
        panic!("claim")
    };
    store
        .complete(invocation, json!({"text":"New result"}))
        .unwrap();
    let after = read(&store);
    assert!(after
        .completed_work
        .iter()
        .any(|item| &item.invocation_ref == invocation && item.sequence > max));
    drop(store);
    let store = ConversationAdmissionStore::open(&path, root).unwrap();
    assert_eq!(read(&store).completed_work, after.completed_work);
}

include!("transfer_legacy_formats.rs");
