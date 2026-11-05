#[test]
fn coworker_create_conversation_owns_a_new_pack_thread_and_retains_exact_retries() {
    use crate::v2026_09::upstream::ConversationContextPolicy;
    let (_temporary, runtime) = participation_runtime();
    let caller = claimed_participation_parent(&runtime);
    let source = caller.conversation_ref.as_deref().unwrap();
    let helper = caller.target_actor_ref.clone();
    runtime
        .configure_conversation_contexts(Some(ConversationContextPolicy {
            action_permissions: Vec::new(),
            suspended: false,
            default_context_ref: "workspace".into(),
            contexts: BTreeMap::from([
                ("workspace".into(), vec![helper.clone()]),
                ("pack".into(), vec![helper]),
                ("disabled".into(), vec![]),
            ]),
        }))
        .unwrap();
    let before = runtime.conversation(source).unwrap().unwrap();
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "create-pack-thread".into(),
        operation: CREATE_CONVERSATION_HOST_OPERATION.into(),
        input: json!({"owner_context_ref":"pack", "text":"@helper Only this task context"}),
    };
    let AdapterHostCallOutcome::Returned(result) =
        runtime.handle_create_conversation(&call, &caller).unwrap();
    assert!(
        result["invocation_ref"].as_str().is_some(),
        "self-addressed creation must schedule fresh work"
    );
    let child = runtime
        .lock()
        .unwrap()
        .admission
        .invocation_records()
        .find(|record| record.invocation_ref == result["invocation_ref"].as_str().unwrap())
        .unwrap()
        .clone();
    assert_eq!(child.target_actor_ref, caller.target_actor_ref);
    assert_ne!(child.invocation_ref, caller.invocation_ref);
    let created = runtime
        .conversation(result["conversation_ref"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(created.owner_context_ref.as_deref(), Some("pack"));
    assert_ne!(created.conversation_ref, source);
    assert_eq!(created.messages.len(), 1);
    assert_eq!(
        created.messages[0].author_actor_ref,
        caller.target_actor_ref
    );
    assert_eq!(runtime.conversation(source).unwrap().unwrap(), before);
    let AdapterHostCallOutcome::Returned(replay) =
        runtime.handle_create_conversation(&call, &caller).unwrap();
    assert_eq!(replay, result);
    let mut changed = call.clone();
    changed.input["owner_context_ref"] = json!("workspace");
    assert!(runtime
        .handle_create_conversation(&changed, &caller)
        .is_err());
    for target in ["disabled", "missing"] {
        let mut denied = call.clone();
        denied.call_id = format!("create-{target}");
        denied.input["owner_context_ref"] = json!(target);
        assert!(runtime
            .handle_create_conversation(&denied, &caller)
            .is_err());
    }
    runtime
        .stop_invocation(&caller.invocation_ref, None)
        .unwrap();
    let mut stopped = call;
    stopped.call_id = "create-after-stop".into();
    assert!(runtime
        .handle_create_conversation(&stopped, &caller)
        .is_err());
}

#[test]
fn coworker_create_conversation_rechecks_stop_after_provider_readiness() {
    let (_temporary, runtime) = participation_runtime();
    let caller = claimed_participation_parent(&runtime);
    let helper = caller.target_actor_ref.clone();
    let binding = runtime
        .graph()
        .unwrap()
        .actors
        .iter()
        .find(|actor| actor.actor_ref == helper)
        .unwrap()
        .default_actor_integration_ref
        .clone()
        .unwrap();
    runtime
        .lock()
        .unwrap()
        .graph
        .actors
        .iter_mut()
        .find(|actor| actor.actor_ref == helper)
        .unwrap()
        .message_binding_preference = vec![binding];
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "create-stop-race".into(),
        operation: CREATE_CONVERSATION_HOST_OPERATION.into(),
        input: json!({"owner_context_ref":"pack", "text":"@helper Task"}),
    };
    let before = runtime.lock().unwrap().admission.admissions().count();
    let ingress = ConversationIngress {
        owner_context_ref: None,
        file_refs: vec![],
        integration_ref: INTRINSIC_CWR_INTEGRATION_REF.into(),
        provider_event_ref: format!("{}#{}", call.invocation_ref, call.call_id),
        author_actor_ref: helper.clone(),
        conversation: ConversationIngressConversation::Create,
        addressing: ConversationIngressAddressing::Addressed {
            actor_refs: vec![helper],
        },
        text: "Task".into(),
    };
    let mut probes = 0;
    let result = runtime.admit_ingress_with_source_input_and_readiness(
        ingress,
        None,
        None,
        None,
        |_| {
            probes += 1;
            runtime
                .stop_invocation(&caller.invocation_ref, None)
                .unwrap();
            AdapterReadiness {
                status: crate::adapter::AdapterReadinessStatus::Ready,
                code: "ready".into(),
                message: "ready".into(),
                repair: None,
            }
        },
        Some((&call, &caller)),
    );
    assert_eq!(probes, 1);
    assert!(result.is_err());
    assert_eq!(
        runtime.lock().unwrap().admission.admissions().count(),
        before
    );
}

#[test]
fn older_provider_conversations_pick_up_creation_once_then_resume() {
    let (_temporary, runtime) = participation_runtime();
    let (first, mut old) = claim_gateway_turn(&runtime, None);
    assert!(old
        .host_operations
        .iter()
        .any(|tool| tool.name == CREATE_CONVERSATION_HOST_OPERATION));
    old.host_operations
        .retain(|tool| tool.name != CREATE_CONVERSATION_HOST_OPERATION);
    runtime
        .lock()
        .unwrap()
        .admission
        .retain_execution_invocation(&first.invocation_ref, &old)
        .unwrap();
    settle_gateway_turn(&runtime, &first, &old);
    let (second, upgraded) = claim_gateway_turn(&runtime, first.conversation_ref.as_deref());
    let context = crate::adapter::working_context(&upgraded.thread).unwrap();
    assert!(context["working_conversation"]["state"].is_null());
    assert!(context["working_conversation"]["history_gap"]
        .as_str()
        .unwrap()
        .contains("host tools changed"));
    assert!(upgraded
        .host_operations
        .iter()
        .any(|tool| tool.name == CREATE_CONVERSATION_HOST_OPERATION));
    settle_gateway_turn(&runtime, &second, &upgraded);
    let (_, resumed) = claim_gateway_turn(&runtime, second.conversation_ref.as_deref());
    assert_eq!(
        serde_json::to_value(&resumed.host_operations).unwrap(),
        serde_json::to_value(&upgraded.host_operations).unwrap()
    );
    let context = crate::adapter::working_context(&resumed.thread).unwrap();
    assert_eq!(
        context["working_conversation"]["state"]["thread_id"],
        "gateway-thread"
    );
}

#[test]
fn coworker_creation_checks_action_and_message_targets_against_caller_allowlist() {
    let (_temporary, runtime) = participation_runtime();
    let other = add_participation_actor(&runtime, "reviewer");
    let caller = claimed_participation_parent(&runtime);
    {
        let mut state = runtime.lock().unwrap();
        state
            .graph
            .actor_integrations
            .iter_mut()
            .find(|binding| {
                Some(&binding.actor_integration_ref) == caller.actor_integration_ref.as_ref()
            })
            .unwrap()
            .actor_integration_params["cwr_action_actor_refs"] = json!([]);
    }
    let before = runtime
        .lock()
        .unwrap()
        .admission
        .invocation_records()
        .count();
    for (index, text) in ["@reviewer--draft--brief {}", "@reviewer Help"]
        .into_iter()
        .enumerate()
    {
        let call = AdapterCall {
            invocation_ref: caller.invocation_ref.clone(),
            call_id: format!("denied-create-{index}"),
            operation: CREATE_CONVERSATION_HOST_OPERATION.into(),
            input: json!({"owner_context_ref":"workspace", "text":text}),
        };
        assert!(runtime
            .handle_create_conversation(&call, &caller)
            .unwrap_err()
            .to_string()
            .contains("not authorized"));
        assert_eq!(
            runtime
                .lock()
                .unwrap()
                .admission
                .invocation_records()
                .count(),
            before
        );
    }
    assert_ne!(other, caller.target_actor_ref);
}
