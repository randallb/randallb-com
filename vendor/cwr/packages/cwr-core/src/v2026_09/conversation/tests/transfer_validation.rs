#[test]
fn conversation_transfer_rejects_invalid_import_without_changes() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("state.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let ConversationAdmissionOutcome::Started(started) = store
        .admit(
            &graph,
            &ingress(
                "invalid-export",
                person,
                None,
                ConversationIngressAddressing::Unaddressed,
                "Source",
            ),
            None,
        )
        .unwrap()
    else {
        panic!("new conversation")
    };
    let export = store
        .export_conversation(&started.conversation.conversation_ref, person)
        .unwrap();
    for invalid in 0..11 {
        let mut modified = export.clone();
        match invalid {
            0 => modified.version = "future-version".into(),
            1 => modified
                .conversation
                .messages
                .push(modified.conversation.messages[0].clone()),
            2 => {
                modified.conversation.messages[0].reply_to_message_ref =
                    Some("missing-message".into())
            }
            3 => modified.conversation.parent_conversation_ref = Some("private".into()),
            4 => modified.conversation.title = Some(" padded ".into()),
            5 => modified.conversation.messages[0].author_actor_ref = "".into(),
            6 => modified.conversation.activity.push(ConversationActivity {
                activity_ref: "missing-source-work".into(),
                sequence: 2,
                occurred_at_unix_ms: modified.exported_at_unix_ms,
                event: ConversationActivityEvent::WorkUpdate {
                    invocation_ref: "missing-source-invocation".into(),
                    source_message_ref: "missing-message".into(),
                    parent_invocation_ref: None,
                    wait: None,
                    actor_ref: person.into(),
                    status: ActionResponseStatus::COMPLETED,
                    text: None,
                    text_kind: None,
                    activity: None,
                },
            }),
            7 => modified.conversation.activity.push(ConversationActivity {
                activity_ref: format!("{}/invalid-lineage", started.conversation.conversation_ref),
                sequence: 2,
                occurred_at_unix_ms: modified.exported_at_unix_ms,
                event: ConversationActivityEvent::ConversationForkedFrom {
                inherited_refs: Default::default(),
                    source_conversation_ref: "https://example.com".into(),
                    source_message_ref: modified.conversation.messages[0].message_ref.clone(),
                    forked_by_actor_ref: person.into(),
                    operation_id: "malicious-lineage".into(),
                    forked_at_unix_ms: modified.exported_at_unix_ms,
                    lineage_depth: 1,
                },
            }),
            8 => {
                let message_ref = modified.conversation.messages[0].message_ref.clone();
                let unrelated_activity_ref = modified.conversation.activity[0].activity_ref.clone();
                modified.conversation.activity.push(ConversationActivity {
                    activity_ref: format!(
                        "{}/invalid-association",
                        started.conversation.conversation_ref
                    ),
                    sequence: 2,
                    occurred_at_unix_ms: modified.exported_at_unix_ms,
                    event: ConversationActivityEvent::MessageWorkAssociation {
                        message_ref,
                        activity_ref: unrelated_activity_ref,
                    },
                });
            }
            9 => modified.conversation.activity.push(ConversationActivity {
                activity_ref: format!(
                    "{}/duplicate-message-boundary",
                    started.conversation.conversation_ref
                ),
                sequence: 2,
                occurred_at_unix_ms: modified.exported_at_unix_ms,
                event: ConversationActivityEvent::MessageRecorded {
                    message_ref: modified.conversation.messages[0].message_ref.clone(),
                },
            }),
            _ => modified.conversation.activity.retain(|item| {
                !matches!(
                    item.event,
                    ConversationActivityEvent::MessageRecorded { .. }
                )
            }),
        }
        assert!(store
            .import_conversation(&graph, person, modified, None)
            .is_err());
        assert_eq!(store.conversations().count(), 1);
    }
    assert!(store
        .import_conversation(&graph, "stranger", export, None)
        .is_err());

    let mut historical = store
        .export_conversation(&started.conversation.conversation_ref, person)
        .unwrap();
    historical.conversation.messages[0].author_actor_ref =
        "cwp://elsewhere.example/actors/dan".into();
    let imported = store
        .import_conversation(&graph, person, historical, None)
        .unwrap();
    assert_eq!(
        imported.messages[0].author_actor_ref,
        "cwp://elsewhere.example/actors/dan"
    );
}

#[test]
fn v1_import_does_not_guess_an_ambiguous_message_work_association() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("ambiguous-v1.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let ConversationAdmissionOutcome::Started(started) = store
        .admit(
            &graph,
            &ingress(
                "ambiguous-v1",
                person,
                None,
                ConversationIngressAddressing::Unaddressed,
                "Source",
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
    export.version = super::transfer::EXPORT_VERSION_V1.into();
    let source_message_ref = export.conversation.messages[0].message_ref.clone();
    export.conversation.record_message(ConversationMessage {
        file_refs: Vec::new(),
        message_ref: format!("{source_message_ref}/reply"),
        author_actor_ref: helper.into(),
        addressed_actor_refs: vec![person.into()],
        addressing_mode: Some(ConversationAddressingMode::Implicit),
        text: "Reply".into(),
        visualizations: Vec::new(),
        reply_to_message_ref: Some(source_message_ref.clone()),
        action: None,
        progress: None,
    });
    for suffix in ["first", "later"] {
        export.conversation.activity.push(ConversationActivity {
            activity_ref: format!("{source_message_ref}/{suffix}-work"),
            sequence: export.conversation.activity.len() as u64 + 1,
            occurred_at_unix_ms: export.exported_at_unix_ms,
            event: ConversationActivityEvent::WorkUpdate {
                invocation_ref: format!("{source_message_ref}/{suffix}-invocation"),
                source_message_ref: source_message_ref.clone(),
                parent_invocation_ref: None,
                wait: None,
                actor_ref: helper.into(),
                status: ActionResponseStatus::COMPLETED,
                text: Some("Reply".into()),
                text_kind: Some(WorkUpdateTextKind::Commentary),
                activity: None,
            },
        });
    }

    let mut missing_association = export.clone();
    missing_association.version = super::transfer::EXPORT_VERSION.into();
    let mut retained_one = false;
    missing_association.conversation.activity.retain(|item| {
        if !matches!(item.event, ConversationActivityEvent::WorkUpdate { .. }) {
            return true;
        }
        if retained_one {
            return false;
        }
        retained_one = true;
        true
    });
    assert!(store
        .import_conversation(&graph, person, missing_association, None)
        .is_err());

    let mut crafted_v2 = export.clone();
    crafted_v2.version = super::transfer::EXPORT_VERSION.into();
    let reply_ref = crafted_v2.conversation.messages[1].message_ref.clone();
    let work_refs = crafted_v2
        .conversation
        .activity
        .iter()
        .filter(|item| matches!(item.event, ConversationActivityEvent::WorkUpdate { .. }))
        .map(|item| item.activity_ref.clone())
        .collect::<Vec<_>>();
    for (index, activity_ref) in work_refs.into_iter().enumerate() {
        crafted_v2.conversation.activity.push(ConversationActivity {
            activity_ref: format!("{source_message_ref}/association-{index}"),
            sequence: crafted_v2.conversation.activity.len() as u64 + 1,
            occurred_at_unix_ms: crafted_v2.exported_at_unix_ms,
            event: ConversationActivityEvent::MessageWorkAssociation {
                message_ref: reply_ref.clone(),
                activity_ref,
            },
        });
    }
    assert!(store
        .import_conversation(&graph, person, crafted_v2, None)
        .is_err());

    let mut inverse = export.clone();
    let imported = store
        .import_conversation(&graph, person, export, None)
        .unwrap();
    assert!(!imported.activity.iter().any(|item| matches!(
        item.event,
        ConversationActivityEvent::MessageWorkAssociation { .. }
    )));

    let mut retained_one = false;
    inverse.conversation.activity.retain(|item| {
        if !matches!(item.event, ConversationActivityEvent::WorkUpdate { .. }) {
            return true;
        }
        if retained_one {
            return false;
        }
        retained_one = true;
        true
    });
    inverse.conversation.record_message(ConversationMessage {
        file_refs: Vec::new(),
        message_ref: format!("{source_message_ref}/reply-two"),
        author_actor_ref: helper.into(),
        addressed_actor_refs: vec![person.into()],
        addressing_mode: Some(ConversationAddressingMode::Implicit),
        text: "Another reply".into(),
        visualizations: Vec::new(),
        reply_to_message_ref: Some(source_message_ref),
        action: None,
        progress: None,
    });
    let inverse_import = store
        .import_conversation(&graph, person, inverse, None)
        .unwrap();
    assert!(!inverse_import.activity.iter().any(|item| matches!(
        item.event,
        ConversationActivityEvent::MessageWorkAssociation { .. }
    )));
}

#[test]
fn imported_fork_uses_retained_message_order_when_activity_order_differs() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("message-order.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let ConversationAdmissionOutcome::Started(first) = store
        .admit(
            &graph,
            &ingress(
                "message-order-first",
                person,
                None,
                ConversationIngressAddressing::Unaddressed,
                "First",
            ),
            None,
        )
        .unwrap()
    else {
        panic!("first message")
    };
    store
        .admit(
            &graph,
            &ingress(
                "message-order-second",
                person,
                Some(&first.conversation.conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "Second",
            ),
            None,
        )
        .unwrap();
    let mut export = store
        .export_conversation(&first.conversation.conversation_ref, person)
        .unwrap();
    export.conversation.activity.reverse();
    let first_message_ref = export.conversation.messages[0].message_ref.clone();
    let reply_ref = format!("{first_message_ref}/reply");
    export.conversation.messages.insert(
        1,
        ConversationMessage {
            file_refs: Vec::new(),
            message_ref: reply_ref.clone(),
            author_actor_ref: helper.into(),
            addressed_actor_refs: vec![person.into()],
            addressing_mode: Some(ConversationAddressingMode::Implicit),
            text: "Earlier reply".into(),
            visualizations: Vec::new(),
            reply_to_message_ref: Some(first_message_ref.clone()),
            action: None,
            progress: None,
        },
    );
    export.conversation.activity.push(ConversationActivity {
        activity_ref: format!("{reply_ref}/recorded"),
        sequence: export.conversation.activity.len() as u64 + 1,
        occurred_at_unix_ms: export.exported_at_unix_ms,
        event: ConversationActivityEvent::MessageRecorded {
            message_ref: reply_ref.clone(),
        },
    });
    let work_ref = format!("{first_message_ref}/completed-work");
    export.conversation.activity.push(ConversationActivity {
        activity_ref: work_ref.clone(),
        sequence: export.conversation.activity.len() as u64 + 1,
        occurred_at_unix_ms: export.exported_at_unix_ms,
        event: ConversationActivityEvent::WorkUpdate {
            invocation_ref: format!("{first_message_ref}/invocation"),
            source_message_ref: first_message_ref,
            parent_invocation_ref: None,
            wait: None,
            actor_ref: helper.into(),
            status: ActionResponseStatus::COMPLETED,
            text: Some("Earlier reply".into()),
            text_kind: Some(WorkUpdateTextKind::Commentary),
            activity: None,
        },
    });
    export.conversation.activity.push(ConversationActivity {
        activity_ref: format!("{reply_ref}/association"),
        sequence: export.conversation.activity.len() as u64 + 1,
        occurred_at_unix_ms: export.exported_at_unix_ms,
        event: ConversationActivityEvent::MessageWorkAssociation {
            message_ref: reply_ref,
            activity_ref: work_ref,
        },
    });
    let imported = store
        .import_conversation(&graph, person, export, None)
        .unwrap();
    let selected_message_ref = imported.messages[2].message_ref.clone();
    let fork = store
        .fork_conversation(
            &graph,
            person,
            &imported.conversation_ref,
            &selected_message_ref,
            "message-order-fork",
        )
        .unwrap();
    assert_eq!(
        fork.messages
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        vec!["First", "Earlier reply", "Second"]
    );
    assert!(fork.activity.iter().any(|item| matches!(
        item.event,
        ConversationActivityEvent::WorkUpdate {
            status: ActionResponseStatus::COMPLETED,
            ..
        }
    )));
}
