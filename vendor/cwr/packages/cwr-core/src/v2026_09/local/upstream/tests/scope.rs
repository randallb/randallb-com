#[test]
fn replayed_admission_keeps_its_original_host_scope() {
    let (_directory, runtime, _actor, _session, _receiver) = fixture();
    let other = UpstreamCoworker {
        actor_ref: "cwp://example.test/tuis/two/actors/other".into(),
        coworker_ref: "cwp://example.test/2026-09/coworkers/other".into(),
        handle: "@other".into(),
        name: "Other".into(),
        actions: vec![crate::v2026_09::upstream::UpstreamAction {
            action_name: "new--conversation".into(),
            title: "New".into(),
            description: "New".into(),
            input_schema: json!({"type":"object","additionalProperties":false}),
            output_schema: json!({"type":"object"}),
        }],
    };
    let session = runtime
        .register_upstream_provider(
            "other",
            vec![other.clone()],
            Arc::new(|_| Ok(())),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    runtime
        .enable_upstream_coworker(
            "other",
            &session,
            &other.actor_ref,
            crate::v2026_09::test_fixture::actor_ref("person"),
            true,
        )
        .unwrap();

    let first = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "replayed",
            crate::v2026_09::test_fixture::actor_ref("person"),
            None,
            "@helper coordinate",
            Some("connection"),
        )
        .unwrap();
    let replay = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "replayed",
            crate::v2026_09::test_fixture::actor_ref("person"),
            None,
            "@helper coordinate",
            Some("other"),
        )
        .unwrap();
    assert_eq!(replay.invocation_ref(), first.invocation_ref());

    let mut claimed = {
        let mut state = runtime.lock().unwrap();
        assert_eq!(
            state
                .host_local_invocation_owners
                .get(first.invocation_ref().unwrap())
                .map(String::as_str),
            Some("connection")
        );
        let ConversationInvocationClaimOutcome::Claimed(claimed) = state
            .admission
            .claim_invocation(first.invocation_ref().unwrap(), 10)
            .unwrap()
        else {
            panic!("model invocation was not runnable")
        };
        claimed
    };
    runtime
        .install_lifecycle_operations(&mut claimed.adapter_invocation, &claimed.claim.invocation)
        .unwrap();
    let operations =
        serde_json::to_string(&claimed.adapter_invocation.thread["coworkers"]).unwrap();
    assert!(operations.contains("@tui--new--conversation"));
    assert!(!operations.contains("@other--new--conversation"));
}

#[test]
fn replay_only_text_requires_and_returns_the_retained_admission() {
    let (_directory, runtime, _actor, _session, _receiver) = fixture();
    let author = crate::v2026_09::test_fixture::actor_ref("person");
    let seed = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "seed",
            author,
            None,
            "@tui seed",
            Some("connection"),
        )
        .unwrap();
    let first = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "stable-continuation",
            author,
            Some(seed.conversation_ref()),
            "@tui continue",
            Some("connection"),
        )
        .unwrap();

    runtime.disconnect_upstream_provider("connection").unwrap();
    assert!(runtime
        .upstream_catalog_as(author, Some("connection"))
        .unwrap()
        .actor_by_handle("@tui")
        .is_none());

    let missing = runtime.replay_text_in_upstream_scope(
        "cwp://example.test/ingress",
        "missing-continuation",
        author,
        seed.conversation_ref(),
        "@tui continue",
        Some("connection"),
    );
    assert!(missing
        .unwrap_err()
        .to_string()
        .contains("has no retained admission"));

    let ordinary_replay = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "stable-continuation",
            author,
            Some(seed.conversation_ref()),
            "@tui continue",
            Some("connection"),
        )
        .unwrap();
    assert_eq!(ordinary_replay.invocation_ref(), first.invocation_ref());

    let replay = runtime
        .replay_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "stable-continuation",
            author,
            seed.conversation_ref(),
            "@tui continue",
            Some("connection"),
        )
        .unwrap();
    assert_eq!(replay.invocation_ref(), first.invocation_ref());
    assert_eq!(
        replay.record.source_message_ref,
        first.record.source_message_ref
    );

    let changed = runtime.replay_text_in_upstream_scope(
        "cwp://example.test/ingress",
        "stable-continuation",
        author,
        seed.conversation_ref(),
        "@tui changed",
        Some("connection"),
    );
    assert!(changed
        .unwrap_err()
        .to_string()
        .contains("different source input"));
}

#[test]
fn replay_only_text_verifies_and_upgrades_a_legacy_admission() {
    let (_directory, runtime, actor, _session, _receiver) = fixture();
    let author = crate::v2026_09::test_fixture::actor_ref("person");
    let seed = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "legacy-seed",
            author,
            None,
            "@tui seed",
            Some("connection"),
        )
        .unwrap();
    let first = runtime
        .admit_ingress_in_scope(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://example.test/ingress".into(),
                provider_event_ref: "legacy-continuation".into(),
                author_actor_ref: author.into(),
                conversation: ConversationIngressConversation::Continue {
                    conversation_ref: seed.conversation_ref().into(),
                },
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec![actor.actor_ref],
                },
                text: "continue".into(),
            },
            None,
            Some("connection"),
        )
        .unwrap();
    assert_eq!(first.record.source_input_fingerprint, None);

    runtime.disconnect_upstream_provider("connection").unwrap();
    let stripped = runtime.replay_text_in_upstream_scope(
        "cwp://example.test/ingress",
        "legacy-continuation",
        author,
        seed.conversation_ref(),
        "continue",
        Some("connection"),
    );
    assert!(stripped
        .unwrap_err()
        .to_string()
        .contains("different source input"));

    let wrong_handle = runtime.replay_text_in_upstream_scope(
        "cwp://example.test/ingress",
        "legacy-continuation",
        author,
        seed.conversation_ref(),
        "@other continue",
        Some("connection"),
    );
    assert!(wrong_handle
        .unwrap_err()
        .to_string()
        .contains("different source input"));

    let replay = runtime
        .replay_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "legacy-continuation",
            author,
            seed.conversation_ref(),
            "@tui continue",
            Some("connection"),
        )
        .unwrap();
    assert_eq!(replay.invocation_ref(), first.invocation_ref());
    assert_eq!(
        replay.record.source_message_ref,
        first.record.source_message_ref
    );
    assert!(replay.record.source_input_fingerprint.is_some());

    let changed = runtime.replay_text_in_upstream_scope(
        "cwp://example.test/ingress",
        "legacy-continuation",
        author,
        seed.conversation_ref(),
        "@tui changed",
        Some("connection"),
    );
    assert!(changed
        .unwrap_err()
        .to_string()
        .contains("different source input"));
}

#[test]
fn send_message_child_inherits_its_callers_host_scope() {
    let (_directory, runtime, _actor, _session, _receiver) = fixture();
    let admission = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "delegating",
            crate::v2026_09::test_fixture::actor_ref("person"),
            None,
            "@helper coordinate",
            Some("connection"),
        )
        .unwrap();
    runtime
        .invite_coworker_as(
            admission.conversation_ref(),
            crate::v2026_09::test_fixture::actor_ref("person"),
            "@cwr",
        )
        .unwrap();
    let caller = {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claimed) = state
            .admission
            .claim_invocation(admission.invocation_ref().unwrap(), 10)
            .unwrap()
        else {
            panic!("model invocation was not runnable")
        };
        claimed.claim.invocation
    };
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "delegate-to-cwr".into(),
        operation: SEND_MESSAGE_HOST_OPERATION.into(),
        input: json!({"to":"@cwr","text":"list actions"}),
    };
    let AdapterHostCallOutcome::Returned(result) = runtime
        .handle_host_call(
            call,
            &caller,
            &Rc::new(RefCell::new(VecDeque::new())),
            &AdapterModelOutputMailbox::default(),
            &mut |_| {},
        )
        .unwrap();
    let response: ActionResponse = serde_json::from_value(result).unwrap();
    assert!(response.result.unwrap()["text"]
        .as_str()
        .unwrap()
        .contains("@tui--new--conversation"));
}

#[test]
fn withdrawing_completed_resource_result_stops_only_its_pending_consuming_root() {
    let (_directory, runtime, _actor, _session, receiver) = fixture();
    let admission = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "mail",
            crate::v2026_09::test_fixture::actor_ref("person"),
            None,
            "@helper coordinate",
            Some("connection"),
        )
        .unwrap();
    let unrelated = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "other",
            crate::v2026_09::test_fixture::actor_ref("person"),
            None,
            "@helper other work",
            Some("connection"),
        )
        .unwrap();
    let caller = {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claim) = state
            .admission
            .claim_invocation(admission.invocation_ref().unwrap(), 10)
            .unwrap()
        else {
            panic!("parent was not runnable")
        };
        claim.claim.invocation
    };
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "read-mail".into(),
        operation: START_ACTION_HOST_OPERATION.into(),
        input: json!({"activation":"@tui--new--conversation", "input":{}}),
    };
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    let AdapterHostCallOutcome::Returned(start) =
        runtime.handle_start_action(&call, &caller, &queue).unwrap();
    let child: StartActionResult = serde_json::from_value(start).unwrap();
    let worker_runtime = runtime.clone();
    let child_ref = child.invocation_ref.clone();
    let worker = thread::spawn(move || {
        worker_runtime.execute_requested_ref(
            &child_ref,
            &AdapterModelOutputMailbox::discard(),
            &mut |_| {},
        )
    });
    let UpstreamEvent::WorkRequested { work } =
        receiver.recv_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("expected mail work")
    };
    assert_eq!(work.root_invocation_ref, caller.invocation_ref);
    runtime
        .complete_upstream_work(
            "connection",
            &child.invocation_ref,
            AdapterTerminal {
                invocation_ref: child.invocation_ref.clone(),
                sequence: 1,
                status: 200,
                summary: "Mail result".into(),
                result: Some(json!({"mail":"private"})),
                error: None,
                continuation: None,
            },
        )
        .unwrap();
    worker.join().unwrap().unwrap();
    assert!(runtime
        .lock()
        .unwrap()
        .admission
        .invocation_record(&child.invocation_ref)
        .unwrap()
        .state
        .is_terminal());
    assert!(!runtime
        .lock()
        .unwrap()
        .admission
        .invocation_record(&caller.invocation_ref)
        .unwrap()
        .state
        .is_terminal());
    assert!(runtime
        .revoke_upstream_result("other-owner", &child.invocation_ref)
        .is_err());
    assert!(runtime
        .revoke_upstream_result("connection", &caller.invocation_ref)
        .is_err());
    let outcome = runtime
        .revoke_upstream_result("connection", &child.invocation_ref)
        .unwrap();
    assert!(matches!(outcome, LocalStopOutcome::Accepted { .. }));
    let state = runtime.lock().unwrap();
    assert_eq!(
        state
            .admission
            .invocation_record(&caller.invocation_ref)
            .unwrap()
            .state,
        ActionInvocationState::Stopping
    );
    assert!(!state
        .admission
        .invocation_record(unrelated.invocation_ref().unwrap())
        .unwrap()
        .state
        .is_terminal());
    assert!(state
        .admission
        .execution_conversation(&caller.invocation_ref)
        .unwrap()
        .messages
        .iter()
        .all(|m| !m.text.contains("private")));
}

#[test]
fn protected_result_is_rechecked_at_parent_reply_publication() {
    for answer in [Some(true), Some(false), None] {
        let allowed = answer.unwrap_or(false);
        let (_directory, runtime, _actor, _session, receiver) = fixture();
        let admission = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                "mail-egress",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                "@helper coordinate",
                Some("connection"),
            )
            .unwrap();
        let caller = {
            let mut state = runtime.lock().unwrap();
            let ConversationInvocationClaimOutcome::Claimed(claim) = state
                .admission
                .claim_invocation(admission.invocation_ref().unwrap(), 10)
                .unwrap()
            else {
                panic!("parent was not runnable")
            };
            claim.claim.invocation
        };
        let call = AdapterCall {
            invocation_ref: caller.invocation_ref.clone(),
            call_id: "read-mail".into(),
            operation: START_ACTION_HOST_OPERATION.into(),
            input: json!({"activation":"@tui--new--conversation","input":{}}),
        };
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        let AdapterHostCallOutcome::Returned(start) =
            runtime.handle_start_action(&call, &caller, &queue).unwrap();
        let child: StartActionResult = serde_json::from_value(start).unwrap();
        let executing = runtime.clone();
        let reference = child.invocation_ref.clone();
        let worker = thread::spawn(move || {
            executing.execute_requested_ref(
                &reference,
                &AdapterModelOutputMailbox::discard(),
                &mut |_| {},
            )
        });
        let UpstreamEvent::WorkRequested { work } =
            receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected work")
        };
        assert!(runtime
            .protect_upstream_result("wrong-owner", &work.invocation_ref)
            .is_err());
        runtime
            .protect_upstream_result("connection", &work.invocation_ref)
            .unwrap();
        runtime
            .complete_upstream_work(
                "connection",
                &work.invocation_ref,
                AdapterTerminal {
                    invocation_ref: work.invocation_ref.clone(),
                    sequence: 1,
                    status: 200,
                    summary: "mail".into(),
                    result: Some(json!({"mail":"private"})),
                    error: None,
                    continuation: None,
                },
            )
            .unwrap();
        worker.join().unwrap().unwrap();
        if answer.is_none() {
            runtime.disconnect_upstream_provider("connection").unwrap();
        }
        let publishing = runtime.clone();
        let parent = caller.clone();
        let publication = thread::spawn(move || {
            publishing.publish_terminal(
                &parent,
                AdapterTerminal {
                    invocation_ref: parent.invocation_ref.clone(),
                    sequence: 1,
                    status: 200,
                    summary: "Private mail answer".into(),
                    result: Some(json!({"text":"Private mail answer"})),
                    error: None,
                    continuation: None,
                },
                &Rc::new(RefCell::new(VecDeque::new())),
                &mut |_| {},
            )
        });
        if answer.is_some() {
            let UpstreamEvent::ValidateResult {
                invocation_ref,
                validation_ref,
            } = receiver.recv_timeout(Duration::from_secs(2)).unwrap()
            else {
                panic!("expected delivery validation")
            };
            assert_eq!(invocation_ref, child.invocation_ref);
            assert!(runtime
                .validate_upstream_result("wrong-owner", &validation_ref, true)
                .is_err());
            runtime
                .validate_upstream_result("connection", &validation_ref, allowed)
                .unwrap();
        }
        publication.join().unwrap().unwrap();
        let state = runtime.lock().unwrap();
        let root = state
            .admission
            .invocation_record(&caller.invocation_ref)
            .unwrap();
        assert_eq!(
            root.state,
            if allowed {
                ActionInvocationState::Completed
            } else {
                ActionInvocationState::Stopped
            }
        );
        let messages = &state
            .admission
            .conversation(admission.conversation_ref())
            .unwrap()
            .messages;
        assert_eq!(
            messages
                .iter()
                .any(|m| m.text.contains("Private mail answer")),
            allowed
        );
        assert!(state.upstream_result_validations.is_empty());
        assert!(state.protected_upstream_results.is_empty());
    }
}
