    #[test]
    fn invalid_adapter_terminal_becomes_a_terminal_failure() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("conversation-admission.json"),
            fixture_host_identity(),
        )
        .unwrap();
        let admission = runtime.admit_text(None, "@helper finish").unwrap();
        let invocation_ref = admission.invocation_ref().unwrap().to_owned();
        let record = {
            let mut state = runtime.lock().unwrap();
            match state
                .admission
                .claim_invocation(&invocation_ref, 10)
                .unwrap()
            {
                ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
                ConversationInvocationClaimOutcome::NotRunnable { .. } => {
                    panic!("new invocation was not runnable")
                }
            }
        };
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        let mut events = Vec::new();
        runtime
            .publish_terminal(
                &record,
                AdapterTerminal {
                    invocation_ref: invocation_ref.clone(),
                    sequence: 1,
                    status: 201,
                    summary: "unsupported".into(),
                    result: None,
                    error: None,
                    continuation: None,
                },
                &queue,
                &mut |event| events.push(event),
            )
            .unwrap();

        let state = runtime.lock().unwrap();
        let retained = state.admission.invocation_record(&invocation_ref).unwrap();
        assert_eq!(retained.state, ActionInvocationState::Failed);
        assert!(!retained.attempt_active);
        assert_eq!(events.len(), 1);
        let LocalRuntimeEvent::Terminal {
            target_actor_ref,
            response,
            message,
        } = &events[0]
        else {
            panic!("execution failure must emit a terminal event")
        };
        assert_eq!(target_actor_ref, &record.target_actor_ref);
        assert_eq!(response.as_ref(), retained.terminal_response().unwrap());
        assert_eq!(response.status, ActionResponseStatus::FAILED);
        assert!(message.is_none());
        assert!(state
            .admission
            .admission_record_for_invocation(&invocation_ref)
            .unwrap()
            .reply_message_ref
            .is_none());
    }

    #[test]
    fn local_catalog_keeps_the_declared_default_among_multiple_bindings() {
        let mut graph = crate::v2026_09::graph::resolve_graph(fixture_workspace()).unwrap();
        let mut remote = graph.actor_integrations[0].clone();
        remote.actor_integration_ref = "cwp://example.test/actor-integrations/helper-remote".into();
        remote.integration_ref = "cwp://example.test/integrations/remote".into();
        remote.adapter_ref = "cwp://example.test/adapters/remote".into();
        graph.actor_integrations.push(remote);

        let catalog = local_catalog(&graph, crate::v2026_09::test_fixture::actor_ref("person")).unwrap();
        assert_eq!(catalog.actors.len(), 1);
        assert_eq!(
            catalog.actors[0].actor_integration_ref,
            graph.actor_integrations[0].actor_integration_ref
        );
    }

    #[test]
    fn explicit_integrated_actor_can_act_for_a_local_host() {
        let mut graph = crate::v2026_09::graph::resolve_graph(fixture_workspace()).unwrap();
        let mut person_integration = graph.actor_integrations[0].clone();
        person_integration.actor_ref = crate::v2026_09::test_fixture::actor_ref("person").into();
        person_integration.actor_integration_ref =
            "cwp://example.test/actor-integrations/person-slack".into();
        person_integration.integration_ref = "cwp://example.test/integrations/slack".into();
        person_integration.adapter_ref = "cwp://example.test/adapters/slack".into();
        graph.actor_integrations.push(person_integration);

        let catalog = local_catalog(&graph, crate::v2026_09::test_fixture::actor_ref("person")).unwrap();
        assert_eq!(catalog.acting_actor.actor_ref, crate::v2026_09::test_fixture::actor_ref("person"));
        assert_eq!(
            catalog
                .known_actor_by_ref(crate::v2026_09::test_fixture::actor_ref("person"))
                .unwrap()
                .handle,
            "@person"
        );
        assert!(catalog
            .actors
            .iter()
            .all(|actor| actor.actor_ref != crate::v2026_09::test_fixture::actor_ref("person")));
        assert_eq!(catalog.actors[0].actor_ref, crate::v2026_09::test_fixture::actor_ref("helper"));
    }

    #[test]
    fn explicit_actor_allows_multiple_host_native_actors_and_no_targets() {
        let mut graph = crate::v2026_09::graph::resolve_graph(fixture_workspace()).unwrap();
        let mut second_person = graph
            .actors
            .iter()
            .find(|actor| actor.actor_ref == crate::v2026_09::test_fixture::actor_ref("person"))
            .unwrap()
            .clone();
        second_person.actor_ref = "~@/actors/reviewer".into();
        second_person.actor_name = Some("Reviewer".into());
        second_person.coworker_slug = "reviewer".into();
        graph.actors.push(second_person);
        graph
            .actors
            .retain(|actor| actor.actor_ref != crate::v2026_09::test_fixture::actor_ref("helper"));
        graph.actor_integrations.clear();
        graph.routes.clear();

        let catalog = local_catalog(&graph, "~@/actors/reviewer").unwrap();
        assert_eq!(catalog.acting_actor.handle, "@reviewer");
        assert_eq!(catalog.known_actors.len(), 2);
        assert_eq!(
            catalog.known_actor_by_ref(crate::v2026_09::test_fixture::actor_ref("person")).unwrap().name,
            "Person"
        );
        assert!(catalog.actors.is_empty());
        assert!(catalog.actions.is_empty());

        let error = default_local_acting_actor_ref(&graph)
            .unwrap_err()
            .to_string();
        assert!(error.contains("requires an explicit acting actor"));
        assert!(error.contains("found 2"));
    }

    #[test]
    fn local_catalog_rejects_a_missing_acting_actor() {
        let graph = crate::v2026_09::graph::resolve_graph(fixture_workspace()).unwrap();
        let error = local_catalog(&graph, "~@/actors/missing")
            .unwrap_err()
            .to_string();
        assert!(error.contains("local acting actor ~@/actors/missing is not in the resolved graph"));
    }

    #[test]
    fn local_catalog_rejects_duplicate_projected_handles() {
        let mut graph = crate::v2026_09::graph::resolve_graph(fixture_workspace()).unwrap();
        let helper = graph
            .actors
            .iter()
            .find(|actor| actor.coworker_slug == "helper")
            .unwrap()
            .clone();
        let helper_integration = graph
            .actor_integrations
            .iter()
            .find(|integration| integration.actor_ref == helper.actor_ref)
            .unwrap()
            .clone();
        let duplicate_ref = "cwp://example.test/actors/person".to_owned();
        let mut duplicate = helper;
        duplicate.coworker_slug = "person".into();
        duplicate.actor_ref = duplicate_ref.clone();
        let mut duplicate_integration = helper_integration;
        duplicate_integration.actor_ref = duplicate_ref.clone();
        duplicate_integration.actor_integration_ref =
            "cwp://example.test/actor-integrations/person".into();
        graph.actors.push(duplicate);
        graph.actor_integrations.push(duplicate_integration);

        let error = local_catalog(&graph, crate::v2026_09::test_fixture::actor_ref("person"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("local handle @person resolves both"));
        assert!(error.contains(&duplicate_ref));
    }

    #[test]
    fn local_catalog_rejects_duplicate_known_actor_handles_without_routes() {
        let mut graph = crate::v2026_09::graph::resolve_graph(fixture_workspace()).unwrap();
        let mut duplicate = graph
            .actors
            .iter()
            .find(|actor| actor.actor_ref == crate::v2026_09::test_fixture::actor_ref("person"))
            .unwrap()
            .clone();
        let duplicate_ref = "cwp://example.test/actors/person".to_owned();
        duplicate.actor_ref = duplicate_ref.clone();
        graph.actors.push(duplicate);

        let error = local_catalog(&graph, crate::v2026_09::test_fixture::actor_ref("person"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("local handle @person resolves both"));
        assert!(error.contains(&duplicate_ref));
    }

    #[test]
    fn failed_reload_after_selected_actor_removal_preserves_the_old_graph_and_catalog() {
        fn copy_directory(source: &Path, destination: &Path) {
            fs::create_dir_all(destination).unwrap();
            for entry in fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                let source = entry.path();
                let destination = destination.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_directory(&source, &destination);
                } else {
                    fs::copy(source, destination).unwrap();
                }
            }
        }

        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        let fixture = fixture_workspace();
        let fixture_root = fixture.parent().unwrap();
        copy_directory(fixture_root, &workspace);
        let manifest = workspace.join("CWR.toml");
        let runtime = LocalRuntime::open_with_state_path(
            &manifest,
            temporary.path().join("conversation-admission.json"),
            catalog_test_host(&manifest),
        )
        .unwrap();
        let before_catalog = runtime.catalog().unwrap();
        let before_graph = runtime.graph().unwrap();
        let contents = fs::read_to_string(&manifest).unwrap();
        let without_person = contents.replace(
            "[[imports]]\npath = \"./coworkers/person/COWORKER.toml\"\nas = \"person\"\n\n",
            "",
        );
        assert_ne!(without_person, contents);
        fs::write(&manifest, without_person).unwrap();

        let admission = runtime
            .submit_text(None, "@cwr reload")
            .unwrap();
        let LocalTextOutcome::Conversation(admission) = admission else {
            panic!("CWR reload must be admitted as conversation work")
        };
        let turn = runtime.execute_admission(&admission, |_| {}).unwrap();
        let response = &turn.conversation.messages.last().unwrap().text;
        assert!(response.contains("CWR graph reload failed"));
        assert!(response.contains("is not in the resolved graph"));
        assert_eq!(runtime.catalog().unwrap(), before_catalog);
        assert_eq!(runtime.graph().unwrap().actors, before_graph.actors);
    }

    #[test]
    fn reload_as_a_connected_actor_survives_startup_actor_removal() {
        fn copy_directory(source: &Path, destination: &Path) {
            fs::create_dir_all(destination).unwrap();
            for entry in fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                let target = destination.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_directory(&entry.path(), &target);
                } else {
                    fs::copy(entry.path(), target).unwrap();
                }
            }
        }

        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        let fixture = fixture_workspace();
        copy_directory(fixture.parent().unwrap(), &workspace);
        let manifest = workspace.join("CWR.toml");
        let runtime = LocalRuntime::open_with_state_path(
            &manifest,
            temporary.path().join("cwpdb.sqlite3"),
            catalog_test_host(&manifest),
        )
        .unwrap();
        let helper_ref = runtime.graph().unwrap().actors.iter().find(|actor| actor.coworker_slug == "helper").unwrap().actor_ref.clone();
        let contents = fs::read_to_string(&manifest).unwrap();
        fs::write(
            &manifest,
            contents.replace(
                "[[imports]]\npath = \"./coworkers/person/COWORKER.toml\"\nas = \"person\"\n\n",
                "",
            ),
        )
        .unwrap();

        let (_, after) = runtime
            .reload_graph_for_connected_actor_validated(&helper_ref, |_, _| Ok(()))
            .expect("a live actor can reload after the startup actor is removed");

        assert_eq!(after.acting_actor.actor_ref, helper_ref);
        assert_eq!(runtime.catalog().unwrap(), after);
    }

    #[test]
    fn action_only_message_completes_from_the_visible_catalog_without_an_adapter() {
        fn copy_directory(source: &Path, destination: &Path) {
            fs::create_dir_all(destination).unwrap();
            for entry in fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                let target = destination.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_directory(&entry.path(), &target);
                } else {
                    fs::copy(entry.path(), target).unwrap();
                }
            }
        }

        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        let fixture = fixture_workspace();
        copy_directory(fixture.parent().unwrap(), &workspace);
        let helper = workspace.join("coworkers/helper");
        fs::write(
            helper.join("action.input.json"),
            r#"{"type":"object","required":["name"],"properties":{"name":{"type":"string"}}}"#,
        )
        .unwrap();
        fs::write(helper.join("action.output.json"), r#"{"type":"object"}"#).unwrap();
        let coworker = helper.join("ROLE.toml");
        fs::write(
            &coworker,
            format!(
                "{}\n[[actions]]\nverb = \"create\"\nobject = \"dev_environment\"\ntitle = \"Create environment\"\ndescription = \"Create one isolated development environment.\"\n[actions.input_schema]\npath = \"./action.input.json\"\n[actions.output_schema]\npath = \"./action.output.json\"\n",
                fs::read_to_string(&coworker).unwrap()
            ),
        )
        .unwrap();
        let binding = helper.join("coworker.toml");
        fs::write(
            &binding,
            format!(
                "{}\nmessage_mode = \"actions_only\"\n",
                fs::read_to_string(&binding).unwrap()
            ),
        )
        .unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            workspace.join("CWR.toml"),
            temporary.path().join("cwpdb.sqlite3"),
            catalog_test_host(&workspace.join("CWR.toml")),
        )
        .unwrap();
        {
            let mut state = runtime.lock().unwrap();
            let mut duplicate = state
                .graph
                .routes
                .iter()
                .find(|route| route.actor_ref == INTRINSIC_CWR_ACTOR_REF)
                .unwrap()
                .clone();
            duplicate.actor_integration_ref =
                "cwp://example.test/actor-integrations/cwr-remote".into();
            state.graph.routes.push(duplicate);
        }
        let admission = runtime
            .admit_text(None, "@helper create a dev environment")
            .unwrap();

        let turn = runtime.execute_admission(&admission, |_| {}).unwrap();

        assert_eq!(turn.invocation.unwrap().state, ActionInvocationState::Completed);
        let reply = turn.conversation.messages.last().unwrap();
        assert!(reply.text.contains("Likely action: `@helper--create--dev_environment"));
        assert!(reply.text.contains("Required inputs: name."));
        assert!(reply.text.contains("@helper--create--dev_environment {\"name\":\"example\"}"));

        let greeting = runtime.admit_text(None, "@helper hello").unwrap();
        let turn = runtime.execute_admission(&greeting, |_| {}).unwrap();
        assert!(!turn
            .conversation
            .messages
            .last()
            .unwrap()
            .text
            .contains("Likely action:"));
    }

fn catalog_test_host(manifest: &Path) -> LocalHostIdentity {
    LocalHostIdentity::new(load_default_local_acting_actor_ref(manifest).unwrap(), "cwp://org.example/integrations/test")
}

    #[test]
    fn reload_stops_removed_coworker_and_preserves_history() {
        fn copy_directory(source: &Path, destination: &Path) {
            fs::create_dir_all(destination).unwrap();
            for entry in fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                let target = destination.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() { copy_directory(&entry.path(), &target); }
                else { fs::copy(entry.path(), target).unwrap(); }
            }
        }
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        copy_directory(fixture_workspace().parent().unwrap(), &workspace);
        let manifest = workspace.join("CWR.toml");
        let runtime = LocalRuntime::open_with_state_path(&manifest, temporary.path().join("state.sqlite3"), catalog_test_host(&manifest)).unwrap();
        let admission = runtime.admit_text(None, "@helper keep this history").unwrap();
        let invocation = admission.invocation_ref().unwrap().to_owned();
        let person = workspace.join("coworkers/person/COWORKER.toml");
        let person_original = fs::read_to_string(&person).unwrap();
        fs::write(&person, person_original.replace("[[imports]]\npath = \"../helper/coworker.toml\"\nas = \"helper\"\n", "")).unwrap();
        let original = fs::read_to_string(&manifest).unwrap();
        let changed = original.replace("[[imports]]\npath = \"./coworkers/helper/coworker.toml\"\nas = \"helper\"\n", "");
        assert_ne!(changed, original);
        fs::write(&manifest, changed).unwrap();
        runtime.reload_graph().unwrap();
        assert!(runtime.lock().unwrap().admission.invocation_record(&invocation).unwrap().state.is_terminal());
        assert!(!runtime.catalog().unwrap().known_actors.iter().any(|actor| actor.handle == "@helper"));
        assert!(runtime.admit_text(None, "@helper new work").is_err());
        drop(runtime);
        let runtime = LocalRuntime::open_with_state_path(&manifest, temporary.path().join("state.sqlite3"), catalog_test_host(&manifest)).unwrap();
        assert!(runtime.admit_text(None, "@helper after restart").is_err());
        assert!(runtime.catalog().unwrap().retained_actors.iter().any(|actor| actor.handle == "@helper"));
        let catalog = runtime.upstream_catalog_as(&catalog_test_host(&manifest).acting_actor_ref, None).unwrap();
        let host = crate::v2026_09::host::project_host_catalog(&catalog);
        assert!(host.retained_actors.iter().any(|actor| actor.handle == "@helper"));
        assert!(!runtime.catalog().unwrap().known_actors.iter().any(|actor| actor.handle == "@helper"));
        fs::write(&manifest, original).unwrap();
        fs::write(&person, person_original).unwrap();
        runtime.reload_graph().unwrap();
        assert!(runtime.admit_text(None, "@helper back again").is_ok());
        assert!(runtime.lock().unwrap().admission.invocation_record(&invocation).is_some());
    }
