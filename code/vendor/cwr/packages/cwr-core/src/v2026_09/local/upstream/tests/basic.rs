    #[test]
    fn disconnect_defers_failure_until_the_conversation_permit_is_released() {
        let (_directory, runtime, _actor, _session, _receiver) = fixture();
        let first = runtime.submit_text_in_upstream_scope(
            "cwp://example.test/ingress", "first", crate::v2026_09::test_fixture::actor_ref("person"), None,
            "@tui new", Some("connection"),
        ).unwrap();
        let second = runtime.submit_text_in_upstream_scope(
            "cwp://example.test/ingress", "second", crate::v2026_09::test_fixture::actor_ref("person"), Some(first.conversation_ref()),
            "@tui new", Some("connection"),
        ).unwrap();
        let permit = runtime.execution.acquire(first.conversation_ref(), || Ok(true)).unwrap();
        runtime.disconnect_upstream_provider("connection").unwrap();
        assert!(!runtime.turn(&first.record).unwrap().invocation.unwrap().state.is_terminal());
        assert!(!runtime.turn(&second.record).unwrap().invocation.unwrap().state.is_terminal());
        drop(permit);
        let mut started = Vec::new();
        runtime.execute_pending(|event| {
            if let LocalRuntimeEvent::InvocationStarted { invocation_ref, .. } = event { started.push(invocation_ref); }
        }).unwrap();
        assert_eq!(started, vec![first.invocation_ref().unwrap(), second.invocation_ref().unwrap()]);
        for admission in [first, second] {
            assert_eq!(runtime.turn(&admission.record).unwrap().invocation.unwrap().state, ActionInvocationState::Failed);
        }
    }

    #[test]
    fn host_local_coworker_does_not_join_the_conversation() {
        let (_directory, runtime, actor, _session, _receiver) = fixture();
        let admission = runtime.submit_text_in_upstream_scope(
            "cwp://example.test/ingress", "private", crate::v2026_09::test_fixture::actor_ref("person"), None,
            "@tui new", Some("connection"),
        ).unwrap();
        assert!(!admission.conversation.participant_actor_refs.contains(&actor.actor_ref));
        assert!(runtime
            .lock()
            .unwrap()
            .admission
            .conversations()
            .all(|conversation| !conversation.participant_actor_refs.contains(&actor.actor_ref)));
        assert!(runtime.upstream_catalog_as(crate::v2026_09::test_fixture::actor_ref("person"), None)
            .unwrap().actor_by_handle("@tui").is_none());
    }

    #[test]
    fn two_hosts_resolve_their_own_tui_and_disconnect_independently() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            super::tests::fixture_workspace(),
            directory.path().join("runtime.sqlite"),
            super::tests::fixture_host_identity(),
        )
        .unwrap();
        let declaration = |instance: &str| UpstreamCoworker {
            actor_ref: format!("cwp://example.test/tuis/{instance}/actors/tui"),
            coworker_ref: "cwp://example.test/2026-09/coworkers/tui".into(),
            handle: "@tui".into(),
            name: "TUI".into(),
            actions: vec![crate::v2026_09::upstream::UpstreamAction {
                action_name: "new--conversation".into(),
                title: "New".into(),
                description: "New".into(),
                input_schema: json!({"type":"object","additionalProperties":false}),
                output_schema: json!({"type":"object"}),
            }],
        };
        let first = declaration("one");
        let second = declaration("two");
        let (first_sender, first_receiver) = std::sync::mpsc::channel();
        let (first_deadline_stops, _first_deadline_stop_events) = std::sync::mpsc::sync_channel(1);
        let first_session = runtime
            .register_upstream_provider(
                "first",
                vec![first.clone()],
                Arc::new(move |event| {
                    first_sender
                        .send(event)
                        .map_err(|error| invalid_error(error.to_string()))
                }),
                first_deadline_stops,
            )
            .unwrap();
        let (second_sender, second_receiver) = std::sync::mpsc::channel();
        let (second_deadline_stops, _second_deadline_stop_events) = std::sync::mpsc::sync_channel(1);
        let second_session = runtime
            .register_upstream_provider(
                "second",
                vec![second.clone()],
                Arc::new(move |event| {
                    second_sender
                        .send(event)
                        .map_err(|error| invalid_error(error.to_string()))
                }),
                second_deadline_stops,
            )
            .unwrap();
        runtime.enable_upstream_coworker(
            "first", &first_session, &first.actor_ref, crate::v2026_09::test_fixture::actor_ref("person"), true,
        ).unwrap();
        runtime.enable_upstream_coworker(
            "second", &second_session, &second.actor_ref, crate::v2026_09::test_fixture::actor_ref("person"), true,
        ).unwrap();

        for (owner, expected, hidden) in [("first", &first, &second), ("second", &second, &first)] {
            let catalog = runtime
                .upstream_catalog_as(
                    crate::v2026_09::test_fixture::actor_ref("person"),
                    Some(owner),
                )
                .unwrap();
            assert_eq!(catalog.actor_by_handle("@tui").unwrap().actor_ref, expected.actor_ref);
            assert!(catalog.actor_by_ref(&hidden.actor_ref).is_none());
        }

        let first_admission = runtime.submit_text_in_upstream_scope(
            "cwp://example.test/ingress", "first", crate::v2026_09::test_fixture::actor_ref("person"), None,
            "@tui--new--conversation", Some("first"),
        ).unwrap();
        let second_admission = runtime.submit_text_in_upstream_scope(
            "cwp://example.test/ingress", "second", crate::v2026_09::test_fixture::actor_ref("person"), None,
            "@tui--new--conversation", Some("second"),
        ).unwrap();
        {
            let state = runtime.lock().unwrap();
            assert_eq!(
                retained_upstream_route(&state, first_admission.invocation_ref().unwrap())
                    .unwrap().coworker.actor_ref,
                first.actor_ref,
            );
            assert_eq!(
                retained_upstream_route(&state, second_admission.invocation_ref().unwrap())
                    .unwrap().coworker.actor_ref,
                second.actor_ref,
            );
        }
        assert!(!first_admission.conversation.participant_actor_refs.contains(&first.actor_ref));
        assert!(!second_admission.conversation.participant_actor_refs.contains(&second.actor_ref));

        let first_runtime = runtime.clone();
        let first_execution = first_admission.clone();
        let first_worker = thread::spawn(move || first_runtime.execute_admission(&first_execution, |_| {}));
        let second_runtime = runtime.clone();
        let second_execution = second_admission.clone();
        let second_worker = thread::spawn(move || second_runtime.execute_admission(&second_execution, |_| {}));
        let UpstreamEvent::WorkRequested { work: first_work } = first_receiver.recv_timeout(Duration::from_secs(2)).unwrap() else { panic!("expected work for the first host"); };
        let UpstreamEvent::WorkRequested { work: second_work } = second_receiver.recv_timeout(Duration::from_secs(2)).unwrap() else { panic!("expected work for the second host"); };
        assert_eq!(first_work.route.provider_session_ref, first_session);
        assert_eq!(first_work.route.coworker.actor_ref, first.actor_ref);
        assert_eq!(second_work.route.provider_session_ref, second_session);
        assert_eq!(second_work.route.coworker.actor_ref, second.actor_ref);
        assert!(first_receiver.try_recv().is_err());
        assert!(second_receiver.try_recv().is_err());

        runtime.disconnect_upstream_provider("first").unwrap();
        assert!(runtime
            .upstream_catalog_as(
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some("first")
            )
            .unwrap()
            .actor_by_handle("@tui")
            .is_none());
        assert_eq!(
            runtime
                .upstream_catalog_as(
                    crate::v2026_09::test_fixture::actor_ref("person"),
                    Some("second")
                )
                .unwrap()
                .actor_by_handle("@tui")
                .unwrap()
                .actor_ref,
            second.actor_ref
        );
        runtime
            .complete_upstream_work(
                "second",
                &second_work.invocation_ref,
                AdapterTerminal {
                    invocation_ref: second_work.invocation_ref.clone(),
                    sequence: 1,
                    status: 200,
                    summary: "Second host completed".into(),
                    result: Some(json!({})),
                    error: None,
                    continuation: None,
                },
            )
            .unwrap();
        assert_eq!(
            first_worker
                .join()
                .unwrap()
                .unwrap()
                .invocation
                .unwrap()
                .state,
            ActionInvocationState::Failed
        );
        assert_eq!(
            second_worker
                .join()
                .unwrap()
                .unwrap()
                .invocation
                .unwrap()
                .state,
            ActionInvocationState::Completed
        );
    }

    #[test]
    fn disabling_a_provider_hides_new_work_without_changing_an_admitted_route() {
        let (_directory, runtime, actor, session, receiver) = fixture();
        let admission = runtime.submit_text_in_upstream_scope(
            "cwp://example.test/ingress", "before-disable", crate::v2026_09::test_fixture::actor_ref("person"), None,
            "@tui--new--conversation", Some("connection"),
        ).unwrap();

        runtime.enable_upstream_coworker(
            "connection", &session, &actor.actor_ref, crate::v2026_09::test_fixture::actor_ref("person"), false,
        ).unwrap();
        assert!(runtime.upstream_catalog_as(crate::v2026_09::test_fixture::actor_ref("person"), Some("connection"))
            .unwrap().actor_by_handle("@tui").is_none());
        assert!(runtime.submit_text_in_upstream_scope(
            "cwp://example.test/ingress", "after-disable", crate::v2026_09::test_fixture::actor_ref("person"), None,
            "@tui--new--conversation", Some("connection"),
        ).is_err());

        let worker_runtime = runtime.clone();
        let worker_admission = admission.clone();
        let worker = thread::spawn(move || worker_runtime.execute_admission(&worker_admission, |_| {}));
        let UpstreamEvent::WorkRequested { work } = receiver.recv_timeout(Duration::from_secs(2)).unwrap() else { panic!("expected the retained route to remain executable"); };
        assert_eq!(work.route.provider_session_ref, session);
        runtime
            .complete_upstream_work(
                "connection",
                &work.invocation_ref,
                AdapterTerminal {
                    invocation_ref: work.invocation_ref.clone(),
                    sequence: 1,
                    status: 200,
                    summary: "Completed retained work".into(),
                    result: Some(json!({})),
                    error: None,
                    continuation: None,
                },
            )
            .unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap().invocation.unwrap().state,
            ActionInvocationState::Completed
        );
    }

    #[test]
    fn first_discovery_in_a_host_conversation_includes_its_provider() {
        let (_directory, runtime, _actor, _session, _receiver) = fixture();
        let admission = runtime.submit_text_in_upstream_scope(
            "cwp://example.test/ingress", "discovery", crate::v2026_09::test_fixture::actor_ref("person"), None,
            "@cwr list coworkers", Some("connection"),
        ).unwrap();
        runtime.execute_admission(&admission, |_| {}).unwrap();
        let conversation = runtime.conversation(admission.conversation_ref()).unwrap().unwrap();
        assert!(conversation.messages.iter().any(|message|
            message.author_actor_ref == INTRINSIC_CWR_ACTOR_REF && message.text.contains("@tui")
        ));
    }

    #[test]
    fn model_execution_in_a_host_conversation_can_invoke_its_provider() {
        let (_directory, runtime, _actor, _session, _receiver) = fixture();
        let admission = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                "model-tools",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                "@helper coordinate",
                Some("connection"),
            )
            .unwrap();
        let mut claimed = {
            let mut state = runtime.lock().unwrap();
            let ConversationInvocationClaimOutcome::Claimed(claimed) = state
                .admission
                .claim_invocation(admission.invocation_ref().unwrap(), 10)
                .unwrap()
            else {
                panic!("model invocation was not runnable")
            };
            claimed
        };

        runtime
            .install_lifecycle_operations(
                &mut claimed.adapter_invocation,
                &claimed.claim.invocation,
            )
            .unwrap();

        let invoke = claimed
            .adapter_invocation
            .host_operations
            .iter()
            .find(|operation| operation.name == INVOKE_ACTION_HOST_OPERATION)
            .expect("model invocation should receive the invoke-action tool");
        assert!(invoke.input_schema["properties"].get("target").is_some());
        let directory = serde_json::to_string(&claimed.adapter_invocation.thread["coworkers"]).unwrap();
        assert!(directory.contains("@tui--new--conversation"));
    }

    include!("scope.rs");
    include!("isolation.rs");
    include!("recovery.rs");

    #[test]
    fn host_coworker_is_admitted_executed_and_retained() {
        let (_directory, runtime, actor, session, receiver) = fixture();
        assert!(runtime.catalog().unwrap().actor_by_handle("@tui").is_none());
        let admission = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                "one",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                "@tui--new--conversation",
                Some("connection"),
            )
            .unwrap();
        let worker_runtime = runtime.clone();
        let worker_admission = admission.clone();
        let worker =
            thread::spawn(move || worker_runtime.execute_admission(&worker_admission, |_| {}));
        let UpstreamEvent::WorkRequested { work } =
            receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected work");
        };
        assert_eq!(work.route.provider_session_ref, session);
        assert_eq!(work.conversation_ref, admission.conversation_ref());
        let terminal = AdapterTerminal {
            invocation_ref: work.invocation_ref.clone(),
            sequence: 1,
            status: 200,
            summary: "Starting a new conversation.".into(),
            result: Some(json!({"text":"Starting a new conversation."})),
            error: None,
            continuation: None,
        };
        assert!(runtime
            .complete_upstream_work("another-connection", &work.invocation_ref, terminal.clone())
            .is_err());
        runtime
            .complete_upstream_work("connection", &work.invocation_ref, terminal.clone())
            .unwrap();
        let turn = worker.join().unwrap().unwrap();
        assert!(runtime.lock().unwrap().upstream_work.is_empty());
        assert_eq!(
            turn.invocation.unwrap().state,
            ActionInvocationState::Completed
        );
        runtime
            .complete_upstream_work("connection", &work.invocation_ref, terminal)
            .unwrap();
        assert!(runtime
            .acknowledge_upstream_stop("connection", &work.invocation_ref)
            .is_err());
        assert_eq!(
            runtime
                .conversation(admission.conversation_ref())
                .unwrap()
                .unwrap()
                .messages
                .len(),
            turn.conversation.messages.len()
        );
        assert!(!turn
            .conversation
            .participant_actor_refs
            .contains(&actor.actor_ref));
        assert!(runtime
            .upstream_catalog_as(crate::v2026_09::test_fixture::actor_ref("person"), Some("another-connection"))
            .unwrap()
            .actor_by_handle("@tui")
            .is_none());
        runtime.disconnect_upstream_provider("connection").unwrap();
        assert!(runtime
            .upstream_catalog_as(crate::v2026_09::test_fixture::actor_ref("person"), None)
            .unwrap()
            .actor_by_handle("@tui")
            .is_none());
    }

    #[test]
    fn disconnect_and_restart_fail_the_original_route() {
        let (directory, runtime, _actor, _session, receiver) = fixture();
        let admission = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                "one",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                "@tui new",
                Some("connection"),
            )
            .unwrap();
        let worker_runtime = runtime.clone();
        let worker_admission = admission.clone();
        let worker =
            thread::spawn(move || worker_runtime.execute_admission(&worker_admission, |_| {}));
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        runtime.disconnect_upstream_provider("connection").unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap().invocation.unwrap().state,
            ActionInvocationState::Failed
        );
        assert!(runtime.lock().unwrap().upstream_work.is_empty());
        drop(runtime);
        let runtime = LocalRuntime::open_with_state_path(
            super::tests::fixture_workspace(),
            directory.path().join("runtime.sqlite"),
            super::tests::fixture_host_identity(),
        )
        .unwrap();
        assert_eq!(
            runtime
                .turn(&admission.record)
                .unwrap()
                .invocation
                .unwrap()
                .state,
            ActionInvocationState::Failed
        );
    }

    #[test]
    fn restart_settles_unfinished_work_without_routing_it_to_a_new_provider() {
        let (directory, runtime, _actor, _session, _receiver) = fixture();
        let admission = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                "one",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                "@tui new",
                Some("connection"),
            )
            .unwrap();
        assert!(runtime.lock().unwrap().upstream_work.is_empty());
        drop(runtime);
        let runtime = LocalRuntime::open_with_state_path(
            super::tests::fixture_workspace(),
            directory.path().join("runtime.sqlite"),
            super::tests::fixture_host_identity(),
        )
        .unwrap();
        assert_eq!(
            runtime
                .turn(&admission.record)
                .unwrap()
                .invocation
                .unwrap()
                .state,
            ActionInvocationState::Failed
        );
        let coworkers = runtime
            .discovery_as(
                Some(admission.conversation_ref()),
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                &IntrinsicCwrCommand::ListCoworkers,
            )
            .unwrap()
            .unwrap();
        assert!(!serde_json::to_string(&coworkers).unwrap().contains("TUI"));
    }

    #[test]
    fn upstream_stop_is_retained_before_provider_acknowledgement() {
        let (_directory, runtime, _actor, _session, receiver) = fixture();
        let admission = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                "one",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                "@tui new",
                Some("connection"),
            )
            .unwrap();
        let worker_runtime = runtime.clone();
        let worker_admission = admission.clone();
        let worker =
            thread::spawn(move || worker_runtime.execute_admission(&worker_admission, |_| {}));
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        let invocation = admission.invocation_ref().unwrap().to_owned();
        let stop_runtime = runtime.clone();
        let stop_ref = invocation.clone();
        let stopping = thread::spawn(move || stop_runtime.stop_invocation(&stop_ref, None));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            UpstreamEvent::StopRequested { .. }
        ));
        assert_eq!(
            runtime
                .turn(&admission.record)
                .unwrap()
                .invocation
                .unwrap()
                .state,
            ActionInvocationState::Stopping
        );
        assert!(runtime
            .acknowledge_upstream_stop("another", &invocation)
            .is_err());
        runtime
            .acknowledge_upstream_stop("connection", &invocation)
            .unwrap();
        assert!(matches!(
            stopping.join().unwrap().unwrap(),
            LocalStopOutcome::Accepted { .. }
        ));
        worker.join().unwrap().unwrap();
        assert_eq!(
            runtime
                .turn(&admission.record)
                .unwrap()
                .invocation
                .unwrap()
                .state,
            ActionInvocationState::Stopped
        );
        assert!(runtime.lock().unwrap().upstream_work.is_empty());
        runtime.fence_upstream_stop_tree(&invocation).unwrap();
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn host_software_presentation_survives_disconnect_without_granting_authority() {
        let (_directory, runtime, actor, _session, _receiver) = fixture();
        let person = crate::v2026_09::test_fixture::actor_ref("person");
        let admission = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                "display",
                person,
                None,
                "@tui new",
                Some("connection"),
            )
            .unwrap();
        let read = || {
            runtime
                .publication_snapshot_until(
                    person,
                    Some("connection"),
                    std::time::Instant::now() + std::time::Duration::from_secs(5),
                    || false,
                    |_| {},
                )
                .unwrap()
        };
        let presentation = read()
            .related_actor(admission.invocation_ref().unwrap(), &actor.actor_ref)
            .unwrap()
            .unwrap();
        assert_eq!(presentation.name.as_deref(), Some("TUI"));
        assert_eq!(presentation.handle.as_deref(), Some("@tui"));
        assert!(presentation.coworker);
        assert!(!admission
            .conversation
            .participant_actor_refs
            .contains(&actor.actor_ref));
        runtime.disconnect_upstream_provider("connection").unwrap();
        assert!(runtime
            .upstream_catalog_as(person, Some("connection"))
            .unwrap()
            .actor_by_ref(&actor.actor_ref)
            .is_none());
        assert_eq!(
            read()
                .related_actor(admission.invocation_ref().unwrap(), &actor.actor_ref)
                .unwrap()
                .unwrap()
                .name
                .as_deref(),
            Some("TUI")
        );
    }
