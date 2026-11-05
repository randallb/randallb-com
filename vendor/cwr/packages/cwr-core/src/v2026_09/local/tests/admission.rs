    #[test]
    fn schema_arguments_parse_scalars_and_inline_json() {
        let schema = json!({
            "type": "object",
            "properties": {
                "question": {"type": "string"},
                "count": {"type": "integer"},
                "enabled": {"type": "boolean"}
            }
        });
        assert_eq!(
            parse_schema_argument_text(
                &schema,
                "--question=how does this work --count=2 --enabled=true"
            )
            .unwrap(),
            json!({"question": "how does this work", "count": 2, "enabled": true})
        );
        assert_eq!(
            parse_schema_argument_text(&schema, r#"{"question":"hello"}"#).unwrap(),
            json!({"question": "hello"})
        );
        assert_eq!(
            parse_schema_argument_text(&schema, "--enabled=false").unwrap(),
            json!({"enabled": false})
        );
        assert_eq!(
            parse_schema_argument_text(&schema, "--question 'separated value' --count 2").unwrap(),
            json!({"question": "separated value", "count": 2})
        );
        assert_eq!(
            parse_schema_argument_text(&schema, "--question '-- urgent' --count 2").unwrap(),
            json!({"question": "-- urgent", "count": 2})
        );
        assert!(
            parse_schema_argument_text(&schema, "--question")
                .unwrap_err()
                .to_string()
                .contains("must be followed by a value")
        );
        assert_eq!(
            parse_schema_arguments::<&str>(
                &json!({"type": "object", "additionalProperties": false}),
                &[],
            )
            .unwrap(),
            json!({})
        );
    }

    #[test]
    fn local_runtime_admits_into_the_2026_09_conversation_store() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();
        let catalog = runtime.catalog().unwrap();
        assert_eq!(catalog.acting_actor.handle, "@person");
        assert_eq!(catalog.actors[0].handle, "@helper");

        let admission = runtime.admit_text(None, "ordinary note").unwrap();
        assert_eq!(admission.record.state, ConversationAdmissionState::Recorded);
        assert_eq!(
            admission.record.integration_ref,
            "cwp://org.coworkerprotocol.local/integrations/test"
        );
        assert!(admission.record.invocation_ref.is_none());
        let conversation = runtime
            .conversation(admission.conversation_ref())
            .unwrap()
            .unwrap();
        assert_eq!(conversation.messages[0].text, "ordinary note");
        assert_eq!(
            conversation.messages[0].author_actor_ref,
            crate::v2026_09::test_fixture::actor_ref("person")
        );
    }

    #[test]
    fn host_exchange_retains_addressed_request_and_authored_reply() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();

        let exchange = runtime
            .record_host_exchange(
                None,
                crate::v2026_09::test_fixture::actor_ref("person"),
                "cwp://com.example.fixture/actors/tui",
                "help".into(),
                "I can help with this TUI.".into(),
            )
            .unwrap();

        assert_eq!(
            exchange.conversation.participant_actor_refs,
            vec![crate::v2026_09::test_fixture::actor_ref("person"), "cwp://com.example.fixture/actors/tui"]
        );
        let request = &exchange.conversation.messages[0];
        assert_eq!(request.author_actor_ref, crate::v2026_09::test_fixture::actor_ref("person"));
        assert_eq!(request.addressed_actor_refs, vec!["cwp://com.example.fixture/actors/tui"]);
        assert_eq!(request.text, "help");
        let response = &exchange.conversation.messages[1];
        assert_eq!(response.author_actor_ref, "cwp://com.example.fixture/actors/tui");
        assert_eq!(response.addressed_actor_refs, vec![crate::v2026_09::test_fixture::actor_ref("person")]);
        assert_eq!(response.reply_to_message_ref, Some(request.message_ref.clone()));

        let retained = runtime
            .conversation(&exchange.conversation.conversation_ref)
            .unwrap()
            .unwrap();
        assert_eq!(retained, exchange.conversation);
    }

    #[test]
    fn host_exchange_cannot_impersonate_a_graph_actor() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();

        let error = runtime
            .record_host_exchange(
                None,
                crate::v2026_09::test_fixture::actor_ref("person"),
                crate::v2026_09::test_fixture::actor_ref("helper"),
                "help".into(),
                "spoofed".into(),
            )
            .unwrap_err();

        assert!(error.to_string().contains("collides with a resolved graph actor"));
    }

    #[test]
    fn actor_integration_action_allowlist_filters_visible_routes() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();
        let admission = runtime.admit_text(None, "@helper coordinate").unwrap();
        let mut state = runtime.lock().unwrap();
        let binding = state
            .graph
            .actor_integrations
            .iter_mut()
            .find(|binding| binding.actor_ref == crate::v2026_09::test_fixture::actor_ref("helper"))
            .unwrap();
        binding.actor_integration_params =
            json!({"cwr_action_actor_refs": [INTRINSIC_CWR_ACTOR_REF]});
        let allowlist = action_actor_allowlist(
            &state.graph,
            crate::v2026_09::test_fixture::actor_ref("helper"),
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .unwrap()
        .unwrap();

        let routes = visible_action_routes(
            &state,
            admission.conversation_ref(),
            crate::v2026_09::test_fixture::actor_ref("helper"),
            Some(&allowlist),
        )
        .unwrap();

        assert!(!routes.is_empty());
        assert!(routes
            .values()
            .all(|route| route.actor_ref == INTRINSIC_CWR_ACTOR_REF));
    }

    #[test]
    fn runtime_cwr_handle_is_case_insensitive() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();

        let outcome = runtime.submit_text(None, "@CWR help").unwrap();
        let LocalTextOutcome::Conversation(admission) = outcome else {
            panic!("CWR help must use normal conversation admission")
        };
        assert_eq!(
            admission.record.target_actor_ref.as_deref(),
            Some(INTRINSIC_CWR_ACTOR_REF)
        );
    }

    #[test]
    fn local_submission_separates_explicit_addressing_from_message_content() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();

        let first = runtime
            .submit_text_as(
                "cwp://org.coworkerprotocol.local/integrations/test",
                "desktop-message-1",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                "  @helper   start here  ",
            )
            .unwrap();
        assert_eq!(first.conversation.messages[0].text, "start here");
        assert_eq!(
            first.conversation.messages[0].addressed_actor_refs,
            vec![crate::v2026_09::test_fixture::actor_ref("helper")]
        );
        assert_eq!(
            first.conversation.messages[0].addressing_mode,
            Some(ConversationAddressingMode::Explicit)
        );

        let second = runtime
            .submit_text_as(
                "cwp://org.coworkerprotocol.local/integrations/test",
                "desktop-message-2",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(first.conversation_ref()),
                "continue implicitly",
            )
            .unwrap();
        assert_eq!(second.conversation.messages[1].text, "continue implicitly");
        assert_eq!(
            second.conversation.messages[1].addressing_mode,
            Some(ConversationAddressingMode::Implicit)
        );
        let statuses = runtime.conversation_turn_statuses().unwrap();
        let statuses = statuses.get(first.conversation_ref()).unwrap();
        assert_eq!(statuses.len(), 2);
        for admission in [&first, &second] {
            let invocation_ref = admission.record.invocation_ref.as_deref().unwrap();
            let status = statuses
                .iter()
                .find(|status| status.invocation_ref == invocation_ref)
                .unwrap();
            assert_eq!(status.source_message_ref, admission.record.source_message_ref);
        }
    }

    #[test]
    fn local_submission_addresses_a_known_actor_without_starting_execution() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();
        {
            let mut state = runtime.lock().unwrap();
            let mut actor = state.graph.actors[0].clone();
            actor.actor_ref = "cwp://com.example.fixture/actors/dan".into();
            actor.actor_name = Some("Dan".into());
            actor.coworker_ref = "cwp://com.example.fixture/coworkers/dan".into();
            actor.coworker_name = Some("Dan".into());
            actor.coworker_slug = "dan".into();
            actor.default_actor_integration_ref = None;
            state.graph.actors.push(actor);

            let mut actor_resource = state
                .graph
                .resources
                .iter()
                .find(|resource| resource.reference == crate::v2026_09::test_fixture::actor_ref("helper"))
                .unwrap()
                .clone();
            actor_resource.reference = "cwp://com.example.fixture/actors/dan".into();
            actor_resource.document["ref"] = json!("cwp://com.example.fixture/actors/dan");
            actor_resource.document["coworker_ref"] = json!("cwp://com.example.fixture/coworkers/dan");
            state.graph.resources.push(actor_resource);
            state.catalog.known_actors.push(LocalActorIdentity {
                actor_ref: "cwp://com.example.fixture/actors/dan".into(),
                coworker_ref: "cwp://com.example.fixture/coworkers/dan".into(),
                handle: "@dan".into(),
                name: "Dan".into(),
                category: None,
                provider_model: None,
            });
        }

        let first = runtime.admit_text(None, "@dan hello from Desktop").unwrap();
        assert_eq!(first.record.state, ConversationAdmissionState::Recorded);
        assert_eq!(first.record.target_actor_ref.as_deref(), Some("cwp://com.example.fixture/actors/dan"));
        assert!(first.record.actor_integration_ref.is_none());
        assert!(first.record.invocation_ref.is_none());
        assert_eq!(first.conversation.messages[0].text, "hello from Desktop");

        let second = runtime
            .admit_text(Some(first.conversation_ref()), "one more thing")
            .unwrap();
        assert_eq!(second.record.state, ConversationAdmissionState::Recorded);
        assert_eq!(second.record.target_actor_ref.as_deref(), Some("cwp://com.example.fixture/actors/dan"));
        assert_eq!(
            second.conversation.messages[1].addressing_mode,
            Some(ConversationAddressingMode::Implicit)
        );

        let coworker = runtime.admit_text(None, "@helper start here").unwrap();
        assert!(coworker.record.invocation_ref.is_some());
        let reference = runtime
            .admit_text(
                Some(coworker.conversation_ref()),
                "this is my friend @dan say hi",
            )
            .unwrap();
        assert_eq!(
            reference.record.target_actor_ref.as_deref(),
            Some(crate::v2026_09::test_fixture::actor_ref("helper"))
        );
        assert!(reference.record.invocation_ref.is_some());
        assert_eq!(
            reference.conversation.messages[1].addressing_mode,
            Some(ConversationAddressingMode::Implicit)
        );
        assert_eq!(
            reference.conversation.messages[1].text,
            "this is my friend @dan say hi"
        );

        let human = runtime
            .admit_text(Some(coworker.conversation_ref()), "@dan say hi")
            .unwrap();
        assert_eq!(human.record.state, ConversationAdmissionState::Recorded);
        assert_eq!(human.record.target_actor_ref.as_deref(), Some("cwp://com.example.fixture/actors/dan"));
        assert!(human.record.actor_integration_ref.is_none());
        assert!(human.record.invocation_ref.is_none());
        assert_eq!(
            human.conversation.messages[2].addressing_mode,
            Some(ConversationAddressingMode::Explicit)
        );
        assert_eq!(human.conversation.messages[2].text, "say hi");

        let coworker_again = runtime
            .admit_text(
                Some(coworker.conversation_ref()),
                "@helper please respond",
            )
            .unwrap();
        assert_eq!(
            coworker_again.record.target_actor_ref.as_deref(),
            Some(crate::v2026_09::test_fixture::actor_ref("helper"))
        );
        assert_eq!(
            coworker_again.record.actor_integration_ref.as_deref(),
            Some(crate::v2026_09::test_fixture::binding_ref("helper"))
        );
        assert!(coworker_again.record.invocation_ref.is_some());
        assert_eq!(
            coworker_again.conversation.messages[3].addressing_mode,
            Some(ConversationAddressingMode::Explicit)
        );
        assert_eq!(coworker_again.conversation.messages[3].text, "please respond");
    }

    #[test]
    fn local_text_submission_retains_runtime_coworker_commands_and_results() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();

        let LocalTextOutcome::Conversation(admission) =
            runtime.submit_text(None, "@cwr list coworkers").unwrap()
        else {
            panic!("expected retained CWR admission")
        };
        assert_eq!(
            admission.record.target_actor_ref.as_deref(),
            Some(INTRINSIC_CWR_ACTOR_REF)
        );
        assert_eq!(admission.conversation.messages[0].text, "list coworkers");
        let turn = runtime.execute_admission(&admission, |_| {}).unwrap();
        assert_eq!(turn.conversation.messages.len(), 2);
        assert_eq!(
            turn.conversation.messages[1].author_actor_ref,
            INTRINSIC_CWR_ACTOR_REF
        );
        assert!(turn.conversation.messages[1]
            .text
            .contains("Available coworkers:"));
        assert!(turn.conversation.messages[1].text.contains("@helper"));

        let LocalTextOutcome::Conversation(admission) =
            runtime.submit_text(None, "ordinary note").unwrap()
        else {
            panic!("expected ordinary conversation admission")
        };
        assert_eq!(admission.conversation.messages[0].text, "ordinary note");

        let retained = runtime
            .admit_conversation_text(None, "@cwr list coworkers")
            .unwrap();
        assert_eq!(
            retained.record.target_actor_ref.as_deref(),
            Some(INTRINSIC_CWR_ACTOR_REF)
        );
    }

    #[test]
    fn local_intrinsic_host_projection_adds_capabilities_and_coworkers() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();
        let host = LocalIntrinsicCwrHost {
            capabilities: IntrinsicCwrHostCapabilities {
                reload_graph: true,
                ..Default::default()
            },
            additional_coworkers: vec![IntrinsicCwrCatalogCoworker {
                details: Default::default(),
                handle: "@desktop".into(),
                name: "CWR Desktop".into(),
                actions: vec!["@desktop--open".into()],
            }],
        };

        let IntrinsicCwrDispatch::Response(IntrinsicCwrResponse::Coworkers { coworkers }) = runtime
            .dispatch_intrinsic_text_for_host("@cwr list coworkers", &host)
            .unwrap()
            .expect("intrinsic host dispatch")
        else {
            panic!("expected projected coworker response")
        };
        assert!(coworkers
            .iter()
            .any(|coworker| coworker.handle == "@desktop"));

        let IntrinsicCwrDispatch::Response(IntrinsicCwrResponse::Actions { actions, input_schemas, .. }) = runtime
            .dispatch_intrinsic_text_for_host("@cwr list actions", &host)
            .unwrap()
            .expect("intrinsic host dispatch")
        else {
            panic!("expected projected action response")
        };
        assert!(actions.contains(&"@desktop--open".into()));
        assert_eq!(input_schemas["@desktop--open"], serde_json::json!({}));

        let IntrinsicCwrDispatch::Response(IntrinsicCwrResponse::Help { actions }) = runtime
            .dispatch_intrinsic_text_for_host("@cwr help", &host)
            .unwrap()
            .expect("intrinsic host dispatch")
        else {
            panic!("expected projected help response")
        };
        assert!(actions.contains(&"@desktop--open".into()));

        assert_eq!(
            runtime.dispatch_intrinsic_text_for_host("@cwr reload", &host).unwrap(),
            Some(IntrinsicCwrDispatch::HostAction(
                crate::intrinsic_cwr::IntrinsicCwrHostAction::ReloadGraph,
            ))
        );
    }

    #[test]
    fn connected_actor_intrinsics_use_that_actors_catalog() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();
        let command = parse_intrinsic_cwr_command("@cwr list coworkers").unwrap();

        let IntrinsicCwrDispatch::Response(IntrinsicCwrResponse::Coworkers { coworkers }) = runtime
            .dispatch_intrinsic_command_as_for_host(
                crate::v2026_09::test_fixture::actor_ref("helper"),
                &command,
                &LocalIntrinsicCwrHost::default(),
            )
            .unwrap()
        else {
            panic!("expected connected actor coworker response")
        };
        let mut handles = coworkers
            .iter()
            .map(|coworker| coworker.handle.as_str())
            .collect::<Vec<_>>();
        handles.sort_unstable();
        assert_eq!(handles, vec!["@cwr", "@helper", "@person"]);
    }

    #[test]
    fn local_runtime_adds_a_newly_addressed_actor_to_a_continued_conversation() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();
        {
            let mut state = runtime.lock().unwrap();
            let mut actor = state.graph.actors[0].clone();
            actor.actor_ref = "cwp://com.example.fixture/actors/reviewer".into();
            actor.actor_name = Some("Reviewer".into());
            actor.coworker_ref = "cwp://com.example.fixture/coworkers/reviewer".into();
            actor.coworker_name = Some("Reviewer".into());
            actor.coworker_slug = "reviewer".into();
            state.graph.actors.push(actor);

            let mut actor_resource = state
                .graph
                .resources
                .iter()
                .find(|resource| resource.reference == crate::v2026_09::test_fixture::actor_ref("helper"))
                .unwrap()
                .clone();
            actor_resource.reference = "cwp://com.example.fixture/actors/reviewer".into();
            actor_resource.document["ref"] = json!("cwp://com.example.fixture/actors/reviewer");
            actor_resource.document["coworker_ref"] = json!("cwp://com.example.fixture/coworkers/reviewer");
            state.graph.resources.push(actor_resource);

            let mut integration = state.graph.actor_integrations[0].clone();
            integration.actor_ref = "cwp://com.example.fixture/actors/reviewer".into();
            integration.actor_name = Some("Reviewer".into());
            integration.coworker_ref = "cwp://com.example.fixture/coworkers/reviewer".into();
            integration.actor_integration_ref =
                "cwp://com.example.fixture/actor_integrations/reviewer_local_execution".into();
            state.graph.actor_integrations.push(integration);
            state.catalog.actors.push(LocalActor {
                actor_ref: "cwp://com.example.fixture/actors/reviewer".into(),
                coworker_ref: "cwp://com.example.fixture/coworkers/reviewer".into(),
                actor_integration_ref: "cwp://com.example.fixture/actor_integrations/reviewer_local_execution".into(),
                handle: "@reviewer".into(),
                name: "Reviewer".into(),
                category: None,
                provider_model: None,
            });
        }

        let first = runtime.admit_text(None, "@helper start").unwrap();
        let second = runtime
            .admit_text(Some(first.conversation_ref()), "@reviewer take a look")
            .unwrap();

        assert_eq!(
            second.conversation.participant_actor_refs,
            vec![crate::v2026_09::test_fixture::actor_ref("person"), crate::v2026_09::test_fixture::actor_ref("helper"), "cwp://com.example.fixture/actors/reviewer",]
        );
        assert_eq!(
            second.record.target_actor_ref.as_deref(),
            Some("cwp://com.example.fixture/actors/reviewer")
        );
    }

    include!("admission_actions.rs");
    include!("admission_titles.rs");
