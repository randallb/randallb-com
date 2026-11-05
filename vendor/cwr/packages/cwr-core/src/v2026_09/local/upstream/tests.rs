#[cfg(test)]
mod upstream_execution_tests {
    use super::*;

    include!("stopping_regressions.rs");

    fn fixture() -> (
        tempfile::TempDir,
        LocalRuntime,
        UpstreamCoworker,
        String,
        std::sync::mpsc::Receiver<UpstreamEvent>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            super::tests::fixture_workspace(),
            directory.path().join("runtime.sqlite"),
            super::tests::fixture_host_identity(),
        )
        .unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let actor = UpstreamCoworker {
            actor_ref: "cwp://example.test/tuis/one/actors/tui".into(),
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
        let (deadline_stops, deadline_stop_events) = std::sync::mpsc::sync_channel(16);
        let stop_sender = sender.clone();
        thread::spawn(move || {
            for event in deadline_stop_events {
                if stop_sender.send(event).is_err() {
                    break;
                }
            }
        });
        let session = runtime
            .register_upstream_provider(
                "connection",
                vec![actor.clone()],
                Arc::new(move |event| {
                    sender
                        .send(event)
                        .map_err(|error| invalid_error(error.to_string()))
                }),
                deadline_stops,
            )
            .unwrap();
        runtime
            .enable_upstream_coworker(
                "connection",
                &session,
                &actor.actor_ref,
                crate::v2026_09::test_fixture::actor_ref("person"),
                true,
            )
            .unwrap();
        (directory, runtime, actor, session, receiver)
    }

    include!("tests/basic.rs");

    include!("archive_tests.rs");
    include!("deadline_tests.rs");
    include!("registration_tests.rs");
    #[test]
    fn another_coworker_cannot_invoke_a_host_local_provider() {
        let (_directory, runtime, _actor, _session, _receiver) = fixture();
        let admission = runtime.admit_text(None, "@helper coordinate").unwrap();
        let parent = {
            let mut state = runtime.lock().unwrap();
            match state
                .admission
                .claim_invocation(admission.invocation_ref().unwrap(), 10)
                .unwrap()
            {
                ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
                _ => panic!("expected a new claim"),
            }
        };
        let call = AdapterCall {
            invocation_ref: parent.invocation_ref.clone(),
            call_id: "new-tui".into(),
            operation: START_ACTION_HOST_OPERATION.into(),
            input: json!({"activation":"@tui--new--conversation", "input":{}}),
        };
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        assert!(runtime.handle_start_action(&call, &parent, &queue).is_err());
        assert!(runtime.handle_start_action(&call, &parent, &queue).is_err());
    }
    #[test]
    fn message_only_coworker_receives_and_completes_addressed_messages() {
        let (_directory, runtime, mut actor, _session, _receiver) = fixture();
        actor.actor_ref = "cwp://example.test/actors/chat".into();
        actor.handle = "@chat".into();
        actor.actions.clear();
        let (sender, receiver) = std::sync::mpsc::channel();
        let session = runtime
            .register_upstream_provider(
                "chat",
                vec![actor.clone()],
                Arc::new(move |event| {
                    sender.send(event).unwrap();
                    Ok(())
                }),
                std::sync::mpsc::sync_channel(1).0,
            )
            .unwrap();
        runtime
            .enable_upstream_coworker(
                "chat",
                &session,
                &actor.actor_ref,
                crate::v2026_09::test_fixture::actor_ref("person"),
                true,
            )
            .unwrap();
        let admission = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                "chat",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                "@chat hello",
                Some("chat"),
            )
            .unwrap();
        let conversation_ref = admission.conversation_ref().to_owned();
        let worker_runtime = runtime.clone();
        let worker = thread::spawn(move || worker_runtime.execute_admission(&admission, |_| {}));
        let UpstreamEvent::WorkRequested { work } = receiver.recv_timeout(Duration::from_secs(2)).unwrap() else { panic!("expected message"); };
        assert_eq!(work.action_name, RESPOND_TO_MESSAGE_ACTION_NAME);
        assert_eq!(work.addressed_message.as_ref().unwrap().text, "hello");
        assert!(runtime
            .complete_upstream_work(
                "chat",
                &work.invocation_ref,
                AdapterTerminal {
                    invocation_ref: work.invocation_ref.clone(),
                    sequence: 1,
                    status: 200,
                    summary: "invalid extra field".into(),
                    result: Some(json!({"text":"Hello", "extra":true})),
                    error: None,
                    continuation: None,
                }
            )
            .is_err());
        assert!(runtime
            .complete_upstream_work(
                "chat",
                &work.invocation_ref,
                AdapterTerminal {
                    invocation_ref: work.invocation_ref.clone(),
                    sequence: 1,
                    status: 200,
                    summary: "invalid blank text".into(),
                    result: Some(json!({"text":"  \n"})),
                    error: None,
                    continuation: None,
                }
            )
            .is_err());
        runtime
            .complete_upstream_work(
                "chat",
                &work.invocation_ref,
                AdapterTerminal {
                    invocation_ref: work.invocation_ref.clone(),
                    sequence: 1,
                    status: 200,
                    summary: "Hello".into(),
                    result: Some(json!({"text":"Hello"})),
                    error: None,
                    continuation: None,
                },
            )
            .unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap().invocation.unwrap().state,
            ActionInvocationState::Completed
        );

        let follow_up = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                "follow-up",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&conversation_ref),
                "@chat current",
                Some("chat"),
            )
            .unwrap();
        let worker_runtime = runtime.clone();
        let worker = thread::spawn(move || worker_runtime.execute_admission(&follow_up, |_| {}));
        let UpstreamEvent::WorkRequested { work } = receiver.recv_timeout(Duration::from_secs(2)).unwrap() else { panic!("expected follow-up message"); };
        assert_eq!(work.addressed_message.as_ref().unwrap().text, "current");
        runtime
            .complete_upstream_work(
                "chat",
                &work.invocation_ref,
                AdapterTerminal {
                    invocation_ref: work.invocation_ref.clone(),
                    sequence: 1,
                    status: 200,
                    summary: "Current".into(),
                    result: Some(json!({"text":"Current"})),
                    error: None,
                    continuation: None,
                },
            )
            .unwrap();
        assert_eq!(
            worker.join().unwrap().unwrap().invocation.unwrap().state,
            ActionInvocationState::Completed
        );
        assert!(runtime.lock().unwrap().upstream_work.is_empty());
    }

}
