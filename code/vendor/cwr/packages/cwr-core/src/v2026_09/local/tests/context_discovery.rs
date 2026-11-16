#[test]
fn coworker_context_discovery_only_exposes_enabled_memberships() {
    use crate::v2026_09::upstream::ConversationContextPolicy;
    let (_temporary, runtime) = participation_runtime();
    let caller = claimed_participation_parent(&runtime);
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "discover-contexts".into(),
        operation: LIST_CONTEXTS_HOST_OPERATION.into(),
        input: json!({}),
    };
    let AdapterHostCallOutcome::Returned(empty) =
        runtime.handle_list_contexts(&call, &caller).unwrap();
    assert_eq!(empty, json!({"contexts":[]}));
    let policy = ConversationContextPolicy {
        action_permissions: Vec::new(),
        suspended: false,
        default_context_ref: "workspace".into(),
        contexts: BTreeMap::from([
            ("workspace".into(), vec![caller.target_actor_ref.clone()]),
            ("pack".into(), vec![caller.target_actor_ref.clone()]),
            ("disabled".into(), vec![]),
            ("unrelated".into(), vec!["cwp://example.test/other".into()]),
        ]),
    };
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    let AdapterHostCallOutcome::Returned(result) =
        runtime.handle_list_contexts(&call, &caller).unwrap();
    assert_eq!(result["contexts"].as_array().unwrap().len(), 2);
    assert_eq!(result["contexts"][0]["owner_context_ref"], "pack");
    assert_eq!(result["contexts"][0]["is_default"], false);
    assert_eq!(result["contexts"][1]["is_default"], true);
    assert_eq!(
        result["contexts"][0]["actor_refs"],
        json!([caller.target_actor_ref])
    );
    let create = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "create-discovered-pack-thread".into(),
        operation: CREATE_CONVERSATION_HOST_OPERATION.into(),
        input: json!({"owner_context_ref":result["contexts"][0]["owner_context_ref"], "text":"@helper Work in this pack"}),
    };
    let AdapterHostCallOutcome::Returned(created) = runtime
        .handle_create_conversation(&create, &caller)
        .unwrap();
    assert_eq!(
        runtime
            .conversation(created["conversation_ref"].as_str().unwrap())
            .unwrap()
            .unwrap()
            .owner_context_ref
            .as_deref(),
        Some("pack")
    );
    runtime
        .configure_conversation_contexts(Some(ConversationContextPolicy {
            action_permissions: Vec::new(),
            suspended: true,
            ..policy
        }))
        .unwrap();
    let AdapterHostCallOutcome::Returned(empty) =
        runtime.handle_list_contexts(&call, &caller).unwrap();
    assert_eq!(empty, json!({"contexts":[]}));
    let mut spoof = call.clone();
    spoof.invocation_ref = "another-invocation".into();
    assert!(runtime.handle_list_contexts(&spoof, &caller).is_err());
    runtime
        .stop_invocation(&caller.invocation_ref, None)
        .unwrap();
    assert!(runtime.handle_list_contexts(&call, &caller).is_err());
}

#[test]
fn older_provider_conversations_pick_up_context_discovery_once_then_resume() {
    let (_temporary, runtime) = participation_runtime();
    let (first, mut old) = claim_gateway_turn(&runtime, None);
    assert!(
        old.host_operations
            .iter()
            .any(|tool| tool.name == LIST_CONTEXTS_HOST_OPERATION)
    );
    old.host_operations
        .retain(|tool| tool.name != LIST_CONTEXTS_HOST_OPERATION);
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
    assert!(
        context["working_conversation"]["history_gap"]
            .as_str()
            .unwrap()
            .contains("host tools changed")
    );
    assert!(
        upgraded
            .host_operations
            .iter()
            .any(|tool| tool.name == LIST_CONTEXTS_HOST_OPERATION)
    );
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
