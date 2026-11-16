#[test]
fn replay_only_text_recovers_a_legacy_first_message_continuation() {
    let (_directory, runtime, actor, _session, _receiver) = fixture();
    let author = crate::v2026_09::test_fixture::actor_ref("person");
    let imported = {
        let mut state = runtime.lock().unwrap();
        // This fixture imports legacy history, so use the disabled writer policy.
        state
            .admission
            .configure_timeline_rollout(
                crate::v2026_09::conversation::ConversationTimelineRollout::Disabled,
            )
            .unwrap();
        let graph = upstream_graph(&state, Some("connection")).unwrap();
        state
            .admission
            .import_conversation(
                &graph,
                author,
                crate::v2026_09::conversation::ConversationExport {
                    timeline: None,
                    version: "cwr.conversation-export/2".into(),
                    exported_at_unix_ms: 1,
                    conversation: RetainedConversation {
                        owner_context_ref: None,
                        conversation_ref: "cwp://example.test/exported/empty".into(),
                        title: None,
                        archived: false,
                        parent_conversation_ref: None,
                        sidebar_eligible: Some(true),
                        participant_actor_refs: vec![author.into()],
                        messages: Vec::new(),
                        timeline_connection_ref: None,
                        activity: Vec::new(),
                    },
                },
                Some("empty-conversation"),
            )
            .unwrap()
    };
    let continuation = runtime
        .admit_ingress_in_scope(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://example.test/ingress".into(),
                provider_event_ref: "legacy-first-continuation".into(),
                author_actor_ref: author.into(),
                conversation: ConversationIngressConversation::Continue {
                    conversation_ref: imported.conversation_ref.clone(),
                },
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec![actor.actor_ref.clone()],
                },
                text: "continue".into(),
            },
            None,
            Some("connection"),
        )
        .unwrap();
    let created = runtime
        .admit_ingress_in_scope(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://example.test/ingress".into(),
                provider_event_ref: "legacy-create".into(),
                author_actor_ref: author.into(),
                conversation: ConversationIngressConversation::Create,
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec![actor.actor_ref],
                },
                text: "create".into(),
            },
            None,
            Some("connection"),
        )
        .unwrap();
    assert_eq!(continuation.conversation.messages.len(), 1);
    assert_eq!(continuation.record.source_input_fingerprint, None);
    assert_eq!(created.record.source_input_fingerprint, None);

    runtime.disconnect_upstream_provider("connection").unwrap();
    let replay = runtime
        .replay_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "legacy-first-continuation",
            author,
            &imported.conversation_ref,
            "@tui continue",
            Some("connection"),
        )
        .unwrap();
    assert_eq!(
        replay.record.source_message_ref,
        continuation.record.source_message_ref
    );
    assert!(replay.record.source_input_fingerprint.is_some());

    let create_as_continuation = runtime.replay_text_in_upstream_scope(
        "cwp://example.test/ingress",
        "legacy-create",
        author,
        created.conversation_ref(),
        "@tui create",
        Some("connection"),
    );
    assert!(create_as_continuation
        .unwrap_err()
        .to_string()
        .contains("different source input"));
}
