#[test]
fn v2_export_infers_a_legacy_terminal_reply_association() {
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
        temporary.path().join("legacy-export.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let ConversationAdmissionOutcome::Started(started) = store
        .admit(
            &graph,
            &ingress(
                "legacy-export",
                person,
                None,
                ConversationIngressAddressing::Unaddressed,
                "Question",
            ),
            None,
        )
        .unwrap()
    else {
        panic!("new conversation")
    };
    let reference = started.conversation.conversation_ref;
    let source_message_ref = started.conversation.messages[0].message_ref.clone();
    let reply_message_ref = format!("{source_message_ref}/legacy-reply");
    store
        .transact(|state| {
            let conversation = state
                .conversations
                .find_mut(|candidate| candidate.conversation_ref == reference)
                .unwrap();
            conversation.record_message(ConversationMessage {
                file_refs: Vec::new(),
                message_ref: reply_message_ref.clone(),
                author_actor_ref: reviewer.clone(),
                addressed_actor_refs: vec![person.into()],
                addressing_mode: Some(ConversationAddressingMode::Implicit),
                text: "Legacy result".into(),
                visualizations: Vec::new(),
                reply_to_message_ref: Some(source_message_ref.clone()),
                action: None,
                progress: None,
            });
            conversation.activity.push(ConversationActivity {
                activity_ref: format!("{source_message_ref}/legacy-work"),
                sequence: conversation.activity.len() as u64 + 1,
                occurred_at_unix_ms: 1,
                event: ConversationActivityEvent::WorkUpdate {
                    invocation_ref: format!("{source_message_ref}/legacy-invocation"),
                    source_message_ref: source_message_ref.clone(),
                    parent_invocation_ref: None,
                    wait: None,
                    actor_ref: reviewer.clone(),
                    status: ActionResponseStatus::COMPLETED,
                    text: Some("Legacy result".into()),
                    text_kind: Some(WorkUpdateTextKind::Commentary),
                    activity: None,
                },
            });
            Ok(())
        })
        .unwrap();

    let export = store.export_conversation(&reference, person).unwrap();
    assert_eq!(export.version, super::transfer::EXPORT_VERSION);
    assert!(export.conversation.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::MessageWorkAssociation { message_ref, .. }
            if message_ref == &reply_message_ref
    )));
    store
        .import_conversation(&graph, person, export, None)
        .unwrap();
}


fn v1_export(mut export: ConversationExport) -> ConversationExport {
    export.version = super::transfer::EXPORT_VERSION_V1.into();
    export.conversation.activity.retain(|item| {
        !matches!(
            item.event,
            ConversationActivityEvent::MessageWorkAssociation { .. }
        )
    });
    export
}

#[test]
fn conversation_transfer_v1_omits_sidebar_state_and_import_is_visible() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_action();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut action = ingress(
        "hidden-export",
        person,
        None,
        ConversationIngressAddressing::Action {
            actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
            activation: "@helper--draft--brief".into(),
            action_name: "draft--brief".into(),
            input: json!({}),
        },
        "@helper--draft--brief",
    );
    action.conversation = ConversationIngressConversation::CreateForAction;
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("sidebar-export.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let ConversationAdmissionOutcome::Started(started) = store
        .admit(
            &graph,
            &action,
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .unwrap()
    else {
        panic!("action export source")
    };
    assert_eq!(started.conversation.sidebar_eligible, Some(false));

    let export = store
        .export_conversation(&started.conversation.conversation_ref, person)
        .unwrap();
    let serialized = serde_json::to_value(export).unwrap();
    assert_eq!(serialized["version"], "cwr.conversation-export/2");
    assert!(serialized["conversation"].get("sidebar_eligible").is_none());
    let v1_export: ConversationExport = serde_json::from_value(serialized).unwrap();
    assert_eq!(v1_export.conversation.sidebar_eligible, None);

    let imported = store
        .import_conversation(&graph, person, v1_export, None)
        .unwrap();
    assert_eq!(imported.sidebar_eligible, Some(true));
    assert!(imported.is_sidebar_eligible());
}

#[test]
fn legacy_conversation_transfer_rejects_invalid_import_without_changes() {
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
    for invalid in 0..8 {
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
            6 => modified.conversation.messages[0]
                .visualizations
                .push(ConversationVisualization {
                    marker_index: 0,
                    title: None,
                    mode: None,
                    artifact_ref: Some("cwp://elsewhere.example/visualizations/private".into()),
                    error: None,
                }),
            _ => modified.conversation.activity.push(ConversationActivity {
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

