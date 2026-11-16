#[test]
fn local_runtime_state_is_owned_until_the_last_clone_drops() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("conversation-admission.json");
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "@helper claimed").unwrap();
    let invocation_ref = admission.invocation_ref().unwrap().to_owned();
    {
        let mut state = runtime.lock().unwrap();
        assert!(matches!(
            state
                .admission
                .claim_invocation(&invocation_ref, 10)
                .unwrap(),
            ConversationInvocationClaimOutcome::Claimed(_)
        ));
    }
    let state_before_second_open = fs::read(&state_path).unwrap();
    let runtime_clone = runtime.clone();

    let second_open = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
    );
    let error = match second_open {
        Ok(_) => panic!("second runtime unexpectedly acquired the state lease"),
        Err(error) => error,
    };
    assert_eq!(
        error.downcast_ref::<io::Error>().map(io::Error::kind),
        Some(io::ErrorKind::WouldBlock)
    );
    assert_eq!(fs::read(&state_path).unwrap(), state_before_second_open);

    drop(runtime);
    assert!(LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity()
    )
    .is_err());
    drop(runtime_clone);

    let recovered = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
    )
    .unwrap();
    let state = recovered.lock().unwrap();
    let invocation = state.admission.invocation_record(&invocation_ref).unwrap();
    assert!(invocation.is_runnable());
    assert!(!invocation.attempt_active);
}

#[test]
fn recovered_roots_are_ordered_by_admission_before_the_current_turn() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("conversation-admission.json");
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
    )
    .unwrap();
    let first = runtime.admit_text(None, "@helper first").unwrap();
    let second = runtime.admit_text(None, "@helper second").unwrap();
    let first_ref = first.invocation_ref().unwrap().to_owned();
    let second_ref = second.invocation_ref().unwrap().to_owned();
    {
        let mut state = runtime.lock().unwrap();
        assert!(matches!(
            state.admission.claim_invocation(&second_ref, 20).unwrap(),
            ConversationInvocationClaimOutcome::Claimed(_)
        ));
        assert!(matches!(
            state.admission.claim_invocation(&first_ref, 10).unwrap(),
            ConversationInvocationClaimOutcome::Claimed(_)
        ));
    }
    drop(runtime);

    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
    )
    .unwrap();
    let current = runtime.admit_text(None, "@helper current").unwrap();
    let current_ref = current.invocation_ref().unwrap().to_owned();
    let ordered = {
        let mut state = runtime.lock().unwrap();
        let ordered = take_ordered_runnable_refs(&mut state, Some(current_ref.clone()));
        assert!(state.pending_invocation_refs.is_empty());
        ordered
    };
    assert_eq!(ordered, vec![first_ref, second_ref, current_ref]);
}

#[test]
fn recovered_invocation_start_is_announced_before_the_current_turn() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("conversation-admission.json");
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
    )
    .unwrap();
    let recovered = runtime.admit_text(None, "@helper recovered").unwrap();
    let recovered_ref = recovered.invocation_ref().unwrap().to_owned();
    {
        let mut state = runtime.lock().unwrap();
        assert!(matches!(
            state
                .admission
                .claim_invocation(&recovered_ref, 10)
                .unwrap(),
            ConversationInvocationClaimOutcome::Claimed(_)
        ));
    }
    drop(runtime);

    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
    )
    .unwrap();
    let current = runtime.admit_text(None, "@helper current").unwrap();
    let current_ref = current.invocation_ref().unwrap().to_owned();
    runtime.lock().unwrap().graph.actor_integrations.clear();
    let mut events = Vec::new();
    runtime
        .execute_admission(&current, |event| events.push(event))
        .unwrap();

    let refs = events
        .iter()
        .map(|event| match event {
            LocalRuntimeEvent::InvocationStarted { invocation_ref, .. } => {
                ("started", invocation_ref.as_str())
            }
            LocalRuntimeEvent::Terminal { response, .. } => {
                ("terminal", response.invocation_ref.as_str())
            }
            LocalRuntimeEvent::Activity { .. } => panic!("route failure emitted activity"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        refs,
        vec![
            ("started", recovered_ref.as_str()),
            ("terminal", recovered_ref.as_str()),
            ("started", current_ref.as_str()),
            ("terminal", current_ref.as_str()),
        ]
    );
}

#[test]
fn legacy_intrinsic_replay_preserves_case_insensitive_cwr_addressing() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let author = crate::v2026_09::test_fixture::actor_ref("person");
    let seed = runtime.admit_text(None, "@helper seed").unwrap();
    let legacy = runtime
        .admit_ingress(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://org.coworkerprotocol.local/integrations/test".into(),
                provider_event_ref: "legacy-cwr-continuation".into(),
                author_actor_ref: author.into(),
                conversation: ConversationIngressConversation::Continue {
                    conversation_ref: seed.conversation_ref().into(),
                },
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec![INTRINSIC_CWR_ACTOR_REF.into()],
                },
                text: "continue".into(),
            },
            None,
        )
        .unwrap();
    assert_eq!(legacy.record.source_input_fingerprint, None);

    let replay = runtime
        .replay_text_in_upstream_scope(
            "cwp://org.coworkerprotocol.local/integrations/test",
            "legacy-cwr-continuation",
            author,
            seed.conversation_ref(),
            "@CWR continue",
            None,
        )
        .unwrap();
    assert_eq!(
        replay.record.source_message_ref,
        legacy.record.source_message_ref
    );
    assert!(replay.record.source_input_fingerprint.is_some());
}

#[test]
fn unavailable_private_history_finishes_the_request_without_repeated_scheduling() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let first = runtime.admit_text(None, "@helper first").unwrap();
    {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claimed) = state
            .admission
            .claim_invocation(first.invocation_ref().unwrap(), 10)
            .unwrap()
        else {
            panic!("claim")
        };
        assert!(claimed.adapter_invocation.conversation_continuity);
        // Simulate losing provider state after a successful public result.
        state
            .admission
            .complete(first.invocation_ref().unwrap(), json!({"text":"Ready"}))
            .unwrap();
    }
    let next = runtime
        .admit_text(Some(first.conversation_ref()), "@helper follow up")
        .unwrap();
    let mut terminals = 0;
    let outcome = runtime
        .execute_admission(&next, |event| {
            if let LocalRuntimeEvent::Terminal { .. } = event {
                terminals += 1;
            }
        })
        .unwrap();
    assert_eq!(terminals, 1);
    let record = outcome.invocation.unwrap();
    assert_eq!(record.state, ActionInvocationState::Failed);
    assert!(record
        .terminal_response()
        .unwrap()
        .error
        .as_ref()
        .unwrap()
        .message
        .as_ref()
        .unwrap()
        .contains("could not be restored"));
    assert!(runtime.try_schedule_execution().unwrap().is_none());
}
