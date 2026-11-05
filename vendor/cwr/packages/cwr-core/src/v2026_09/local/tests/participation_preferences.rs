#[test]
fn preferred_messages_reselect_only_for_new_work_and_explicit_targets_never_fall_back() {
    let (_temporary, runtime) = participation_runtime();
    let helper = participation_ref(&runtime, "helper");
    let (primary, secondary) = {
        let mut state = runtime.lock().unwrap();
        let first = state
            .graph
            .actor_integrations
            .iter_mut()
            .find(|b| b.actor_ref == helper)
            .unwrap();
        first.adapter_ref = "cwp://example/adapters/unavailable".into();
        let primary = first.actor_integration_ref.clone();
        let mut second = first.clone();
        second.actor_integration_ref = format!("{helper}/bindings/secondary");
        second.adapter_ref = INTRINSIC_CWR_ADAPTER_REF.into();
        let secondary = second.actor_integration_ref.clone();
        state.graph.actor_integrations.push(second);
        state
            .graph
            .actors
            .iter_mut()
            .find(|a| a.actor_ref == helper)
            .unwrap()
            .message_binding_preference = vec![primary.clone(), secondary.clone()];
        (primary, secondary)
    };
    let first = runtime.admit_text(None, "@helper first").unwrap();
    assert_eq!(
        first.record.actor_integration_ref.as_ref(),
        Some(&secondary)
    );
    let queued = runtime
        .admit_text(Some(first.conversation_ref()), "followup")
        .unwrap();
    assert_eq!(
        queued.record.actor_integration_ref.as_ref(),
        Some(&secondary)
    );
    {
        let mut state = runtime.lock().unwrap();
        state
            .graph
            .actor_integrations
            .iter_mut()
            .find(|b| b.actor_integration_ref == primary)
            .unwrap()
            .adapter_ref = INTRINSIC_CWR_ADAPTER_REF.into();
    }
    let next = runtime
        .admit_text(Some(first.conversation_ref()), "new work")
        .unwrap();
    assert_eq!(next.record.actor_integration_ref.as_ref(), Some(&primary));
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(queued.record.invocation_ref.as_deref().unwrap())
            .unwrap()
            .actor_integration_ref
            .as_ref(),
        Some(&secondary)
    );
    {
        let mut state = runtime.lock().unwrap();
        for binding in state
            .graph
            .actor_integrations
            .iter_mut()
            .filter(|b| b.actor_ref == helper)
        {
            binding.adapter_ref = "cwp://example/adapters/unavailable".into();
        }
    }
    let error = runtime
        .admit_text(Some(first.conversation_ref()), "needs setup")
        .unwrap_err();
    assert!(error.to_string().contains("ready connection"));
    let explicit = runtime
        .admit_ingress(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://org.example/host".into(),
                provider_event_ref: "explicit".into(),
                author_actor_ref: participation_ref(&runtime, "person"),
                conversation: ConversationIngressConversation::Create,
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec![helper],
                },
                text: "exact primary".into(),
            },
            Some(&primary),
        )
        .unwrap();
    // Existing explicit-target behavior admits the exact route; execution will
    // report its readiness failure instead of changing providers.
    assert_eq!(
        explicit.record.actor_integration_ref.as_ref(),
        Some(&primary)
    );
}

#[test]
fn preference_preflights_release_runtime_lock_and_probe_each_candidate_once() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    mark_actor_ready_for_test(&runtime, &reviewer);
    let caller = claimed_participation_parent(&runtime);
    let (primary, secondary) = {
        let mut state = runtime.lock().unwrap();
        let mut binding = state
            .graph
            .actor_integrations
            .iter()
            .find(|b| b.actor_ref == reviewer)
            .unwrap()
            .clone();
        let primary = binding.actor_integration_ref.clone();
        binding.actor_integration_ref = format!("{reviewer}/bindings/secondary");
        let secondary = binding.actor_integration_ref.clone();
        state.graph.actor_integrations.push(binding);
        state
            .graph
            .actors
            .iter_mut()
            .find(|a| a.actor_ref == reviewer)
            .unwrap()
            .message_binding_preference = vec![primary, secondary.clone()];
        (
            state
                .graph
                .actors
                .iter()
                .find(|a| a.actor_ref == reviewer)
                .unwrap()
                .message_binding_preference[0]
                .clone(),
            secondary,
        )
    };
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "preflight".into(),
        operation: SEND_MESSAGE_HOST_OPERATION.into(),
        input: json!({}),
    };
    let input = SendMessageInput {
        runtime_ref: None,
        binding_ref: None,
        to: reviewer.clone(),
        text: "help".into(),
    };
    let mut count = 0;
    let checked = runtime
        .message_preflight_with_readiness(&input, &call, &caller, |route| {
            assert!(
                runtime.inner.try_lock().is_ok(),
                "probe must not block runtime operations"
            );
            count += 1;
            let mut status = route.readiness();
            if count == 1 {
                status.status = crate::adapter::AdapterReadinessStatus::SetupRequired;
            }
            status
        })
        .unwrap()
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(checked.binding_ref, secondary);
    let graph = runtime.graph().unwrap();
    let selected = graph
        .actor_integrations
        .iter()
        .find(|b| b.actor_integration_ref == secondary)
        .unwrap();
    validate_message_preflight(
        &graph,
        selected,
        &checked,
        &[primary.clone(), secondary.clone()],
    )
    .unwrap();
    let mut changed_primary = graph.clone();
    changed_primary
        .actor_integrations
        .iter_mut()
        .find(|binding| binding.actor_integration_ref == primary)
        .unwrap()
        .adapter_ref = "cwp://example/changed-primary".into();
    assert!(validate_message_preflight(
        &changed_primary,
        selected,
        &checked,
        &[primary.clone(), secondary.clone()]
    )
    .is_err());
    assert!(validate_message_preflight(
        &graph,
        selected,
        &checked,
        &[secondary.clone(), primary.clone()]
    )
    .is_err());
    assert!(validate_message_preflight(&graph, selected, &checked, &[]).is_err());
    let exact = SendMessageInput {
        binding_ref: Some(secondary.clone()),
        ..input
    };
    let exact_checked = runtime
        .message_preflight_with_readiness(&exact, &call, &caller, |route| route.readiness())
        .unwrap()
        .unwrap();
    validate_message_preflight(&graph, selected, &exact_checked, &[]).unwrap();
    let mut changed = selected.clone();
    changed.adapter_ref = "cwp://example/changed".into();
    assert!(validate_message_preflight(
        &graph,
        &changed,
        &checked,
        &[primary.clone(), secondary.clone()]
    )
    .is_err());

    let ingress = ConversationIngress {
        owner_context_ref: None,
        file_refs: Vec::new(),
        integration_ref: "cwp://org.example/host".into(),
        provider_event_ref: "unlocked".into(),
        author_actor_ref: participation_ref(&runtime, "person"),
        conversation: ConversationIngressConversation::Create,
        addressing: ConversationIngressAddressing::Addressed {
            actor_refs: vec![reviewer],
        },
        text: "new message".into(),
    };
    let admitted = runtime
        .admit_ingress_with_readiness(ingress.clone(), None, None, |route| {
            assert!(
                runtime.inner.try_lock().is_ok(),
                "ingress probe must release runtime lock"
            );
            route.readiness()
        })
        .unwrap();
    assert_eq!(
        admitted.record.actor_integration_ref.as_ref(),
        Some(&primary)
    );
    let replay = runtime
        .admit_ingress_with_readiness(ingress, None, None, |_| panic!("replay must not probe"))
        .unwrap();
    assert_eq!(replay.record.invocation_ref, admitted.record.invocation_ref);
}

#[test]
fn preference_ingress_rejects_route_changes_during_unlocked_probe() {
    let (_temporary, runtime) = participation_runtime();
    let helper = participation_ref(&runtime, "helper");
    mark_actor_ready_for_test(&runtime, &helper);
    let (primary, secondary) = {
        let mut state = runtime.lock().unwrap();
        let first = state
            .graph
            .actor_integrations
            .iter()
            .find(|binding| binding.actor_ref == helper)
            .unwrap()
            .clone();
        let primary = first.actor_integration_ref.clone();
        let mut fallback = first;
        fallback.actor_integration_ref = format!("{helper}/bindings/fallback");
        let secondary = fallback.actor_integration_ref.clone();
        state.graph.actor_integrations.push(fallback);
        let actor = state
            .graph
            .actors
            .iter_mut()
            .find(|a| a.actor_ref == helper)
            .unwrap();
        actor.message_binding_preference = vec![primary.clone(), secondary.clone()];
        (primary, secondary)
    };
    let ingress = ConversationIngress {
        owner_context_ref: None,
        file_refs: Vec::new(),
        integration_ref: "cwp://org.example/host".into(),
        provider_event_ref: "changed".into(),
        author_actor_ref: participation_ref(&runtime, "person"),
        conversation: ConversationIngressConversation::Create,
        addressing: ConversationIngressAddressing::Addressed {
            actor_refs: vec![helper.clone()],
        },
        text: "new message".into(),
    };
    let mut probes = 0;
    let result = runtime.admit_ingress_with_readiness(ingress, None, None, |route| {
        probes += 1;
        let mut status = route.readiness();
        if probes == 1 {
            status.status = crate::adapter::AdapterReadinessStatus::SetupRequired;
        } else {
            runtime
                .lock()
                .unwrap()
                .graph
                .actor_integrations
                .iter_mut()
                .find(|binding| binding.actor_integration_ref == primary)
                .unwrap()
                .adapter_ref = "cwp://example/changed".into();
        }
        status
    });
    assert_eq!(probes, 2);
    assert!(result.unwrap_err().to_string().contains("route changed"));
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_records()
            .count(),
        0
    );
    assert_ne!(primary, secondary);
}

#[test]
fn concurrent_ingress_replay_wins_over_a_stale_readiness_error() {
    let (_temporary, runtime) = participation_runtime();
    let helper = participation_ref(&runtime, "helper");
    mark_actor_ready_for_test(&runtime, &helper);
    {
        let mut state = runtime.lock().unwrap();
        let actor = state
            .graph
            .actors
            .iter_mut()
            .find(|a| a.actor_ref == helper)
            .unwrap();
        actor.message_binding_preference =
            vec![actor.default_actor_integration_ref.clone().unwrap()];
    }
    let ingress = ConversationIngress {
        owner_context_ref: None,
        file_refs: Vec::new(),
        integration_ref: "cwp://org.example/host".into(),
        provider_event_ref: "concurrent-replay".into(),
        author_actor_ref: participation_ref(&runtime, "person"),
        conversation: ConversationIngressConversation::Create,
        addressing: ConversationIngressAddressing::Addressed {
            actor_refs: vec![helper],
        },
        text: "new message".into(),
    };
    let mut admitted = None;
    let replay = runtime
        .admit_ingress_with_readiness(ingress.clone(), None, None, |route| {
            admitted = Some(
                runtime
                    .admit_ingress_with_readiness(ingress.clone(), None, None, |route| {
                        route.readiness()
                    })
                    .unwrap(),
            );
            let mut status = route.readiness();
            status.status = crate::adapter::AdapterReadinessStatus::SetupRequired;
            status.message = "stale setup requirement".into();
            status
        })
        .unwrap();
    assert_eq!(
        replay.record.invocation_ref,
        admitted.unwrap().record.invocation_ref
    );
}

