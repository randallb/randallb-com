#[test]
fn conversation_transfer_replaces_visualizations_with_readable_text() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("visualization-export.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let ConversationAdmissionOutcome::Started(started) = store
        .admit(
            &graph,
            &ingress(
                "visualization-export",
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
    let reference = started.conversation.conversation_ref;
    let artifact_ref = "cwp://com.example.runtime/visualizations/private";
    store
        .transact(|state| {
            let message = &mut state
                .conversations
                .find_mut(|conversation| conversation.conversation_ref == reference)
                .unwrap()
                .messages[0];
            message.text = "Before\n\n\u{e200}visualize\u{e202}{\"path\":\"/Users/private/chart.html\",\"title\":\"Saved chart\"}\u{e201}\n\nAfter".into();
            message.visualizations = vec![ConversationVisualization {
                marker_index: 0,
                title: Some("Saved chart".into()),
                mode: None,
                artifact_ref: Some(artifact_ref.into()),
                error: None,
            }];
            state.visualization_artifacts.insert(
                artifact_ref.into(),
                ConversationVisualizationArtifact {
                    artifact_ref: artifact_ref.into(),
                    html: "<p>private</p>".into(),
                },
            );
            Ok(())
        })
        .unwrap();

    let export = store.export_conversation(&reference, person).unwrap();
    let serialized = serde_json::to_string(&export).unwrap();
    assert_eq!(export.conversation.messages[0].text, "Before\n\nSaved chart\n\nAfter");
    assert!(export.conversation.messages[0].visualizations.is_empty());
    assert!(!serialized.contains("/Users/private"));
    assert!(!serialized.contains(artifact_ref));
    assert!(!serialized.contains("<p>private</p>"));

    let imported = store
        .import_conversation(&graph, person, export, None)
        .unwrap();
    assert!(imported.messages[0].visualizations.is_empty());

    let source_message_ref = store.conversation(&reference).unwrap().messages[0]
        .message_ref
        .clone();
    let fork = store
        .fork_conversation(
            &graph,
            person,
            &reference,
            &source_message_ref,
            "visualization-fork",
        )
        .unwrap();
    assert_eq!(fork.messages[0].text, "Before\n\nSaved chart\n\nAfter");
    assert!(fork.messages[0].visualizations.is_empty());
    drop(store);
    ConversationAdmissionStore::open(
        temporary.path().join("visualization-export.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
}
