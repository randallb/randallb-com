fn coworker_is_routable(value: Value, actor_ref: &str) -> bool {
    let IntrinsicCwrResponse::Coworkers { coworkers } =
        serde_json::from_value::<IntrinsicCwrResponse>(value).unwrap()
    else {
        panic!("missing coworker list")
    };
    coworkers.iter().any(|coworker| {
        coworker.details["coworker_ref"] == actor_ref
            && coworker
                .details
                .get("bindings")
                .and_then(Value::as_array)
                .is_some_and(|bindings| !bindings.is_empty())
    })
}

fn list_coworkers_for_invocation(
    runtime: &LocalRuntime,
    caller: &ActionInvocationRecord,
    call_id: &str,
) -> Value {
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: call_id.into(),
        operation: LIST_COWORKERS_HOST_OPERATION.into(),
        input: json!({}),
    };
    let AdapterHostCallOutcome::Returned(value) =
        runtime.handle_discovery_call(&call, caller).unwrap();
    value
}

fn claim_private_invocation(
    runtime: &LocalRuntime,
    invocation_ref: &str,
) -> (ActionInvocationRecord, AdapterInvocation) {
    let mut state = runtime.lock().unwrap();
    match state
        .admission
        .claim_invocation(invocation_ref, current_unix_ms())
        .unwrap()
    {
        ConversationInvocationClaimOutcome::Claimed(claimed) => {
            (claimed.claim.invocation, claimed.adapter_invocation)
        }
        _ => panic!("specialist action was not claimed"),
    }
}

#[test]
fn manager_can_discover_and_message_private_coworker_without_public_membership() {
    use crate::v2026_09::upstream::{ContextActionPermission, ConversationContextPolicy};

    let (_temporary, runtime) = participation_runtime();
    let specialist = add_participation_actor(&runtime, "specialist");
    let manager = claimed_participation_parent(&runtime);
    let manager_ref = manager.target_actor_ref.clone();
    let person = participation_ref(&runtime, "person");
    let conversation_ref = manager.conversation_ref.as_deref().unwrap();
    let mut policy = ConversationContextPolicy {
        suspended: false,
        default_context_ref: "workspace".into(),
        contexts: BTreeMap::from([("workspace".into(), vec![manager_ref.clone()])]),
        action_permissions: vec![
            ContextActionPermission {
                caller_actor_ref: manager_ref.clone(),
                target_actor_ref: specialist.clone(),
                action_names: vec![RESPOND_TO_MESSAGE_ACTION_NAME.into(), "draft--brief".into()],
            },
            ContextActionPermission {
                caller_actor_ref: specialist.clone(),
                target_actor_ref: manager_ref.clone(),
                action_names: vec![RESPOND_TO_MESSAGE_ACTION_NAME.into()],
            },
        ],
    };
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    assert_eq!(
        runtime.conversation_context_targets(None).unwrap(),
        Some(vec![manager_ref.clone()])
    );
    assert!(runtime
        .admit_text(None, "@specialist Direct person request")
        .is_err());

    let coworkers = |caller: &str| {
        let Some(IntrinsicCwrResponse::Coworkers { coworkers }) = runtime
            .discovery_as(
                Some(conversation_ref),
                caller,
                None,
                &IntrinsicCwrCommand::ListCoworkers,
            )
            .unwrap()
        else {
            panic!("missing coworker discovery")
        };
        coworkers
    };
    assert!(coworkers(&manager_ref)
        .iter()
        .any(|coworker| coworker.handle == "@specialist"));
    assert!(!coworkers(&person)
        .iter()
        .any(|coworker| coworker.handle == "@specialist"));

    let input = SendMessageInput {
        runtime_ref: None,
        binding_ref: None,
        to: specialist.clone(),
        text: "Please review".into(),
    };
    let call = AdapterCall {
        invocation_ref: manager.invocation_ref.clone(),
        call_id: "manager-specialist-message".into(),
        operation: SEND_MESSAGE_HOST_OPERATION.into(),
        input: json!({"to": specialist, "text": "Please review"}),
    };
    assert!(runtime
        .message_preflight_with_readiness(
            &input,
            &call,
            &manager,
            |_: &crate::adapter::ResolvedProcessAdapterRoute| AdapterReadiness {
                status: crate::adapter::AdapterReadinessStatus::Ready,
                code: "ready".into(),
                message: "ready".into(),
                repair: None,
            }
        )
        .unwrap()
        .is_some());

    // The same scoped policy exposes exact private action routes to the
    // manager without making those routes available to the person.
    mark_actor_ready_for_test(&runtime, &specialist);
    let action = list_actions_for(&runtime, &manager, json!({"coworker_ref": specialist})).unwrap();
    let target = action["actions"][0]["target"].clone();
    assert_eq!(target["action_name"], "draft--brief");
    {
        let state = runtime.lock().unwrap();
        assert!(state
            .admission
            .check_context_work(
                &state.graph,
                None,
                true,
                &person,
                Some((&specialist, Some("draft--brief"))),
                None,
            )
            .is_err());
    }
    let action_call = AdapterCall {
        invocation_ref: manager.invocation_ref.clone(),
        call_id: "manager-specialist-action".into(),
        operation: INVOKE_ACTION_HOST_OPERATION.into(),
        input: json!({"target": target, "input": {}}),
    };
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    runtime
        .handle_start_action(&action_call, &manager, &queue)
        .unwrap();
    let child_ref = queue.borrow_mut().pop_front().unwrap();
    {
        let state = runtime.lock().unwrap();
        let child = state.admission.invocation_record(&child_ref).unwrap();
        assert_eq!(child.target_actor_ref, specialist);
        assert!(state
            .admission
            .check_context_execution(&state.graph, child)
            .is_ok());
    }

    // The private specialist's real child invocation can message its caller.
    // The current policy must authorize both the retained parent and the reply.
    let (specialist_caller, mut specialist_invocation) =
        claim_private_invocation(&runtime, &child_ref);
    // Discovery must use this child invocation's authorized parent chain.
    // Without it, the private specialist cannot see its manager's binding.
    runtime
        .install_lifecycle_operations(&mut specialist_invocation, &specialist_caller)
        .unwrap();
    assert!(coworker_is_routable(
        specialist_invocation.thread["coworkers"].clone(),
        &manager_ref
    ));
    assert!(coworker_is_routable(
        list_coworkers_for_invocation(&runtime, &specialist_caller, "specialist-list-coworkers"),
        &manager_ref
    ));
    let reply_input = SendMessageInput {
        runtime_ref: None,
        binding_ref: None,
        to: manager_ref.clone(),
        text: "Review complete".into(),
    };
    let reply_call = AdapterCall {
        invocation_ref: specialist_caller.invocation_ref.clone(),
        call_id: "specialist-manager-message".into(),
        operation: SEND_MESSAGE_HOST_OPERATION.into(),
        input: json!({"to": manager_ref, "text": "Review complete"}),
    };
    assert!(runtime
        .message_preflight_with_readiness(
            &reply_input,
            &reply_call,
            &specialist_caller,
            |_: &crate::adapter::ResolvedProcessAdapterRoute| AdapterReadiness {
                status: crate::adapter::AdapterReadinessStatus::Ready,
                code: "ready".into(),
                message: "ready".into(),
                repair: None,
            },
        )
        .unwrap()
        .is_some());
    policy.action_permissions.pop();
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    assert!(!coworker_is_routable(
        list_coworkers_for_invocation(
            &runtime,
            &specialist_caller,
            "specialist-list-after-revocation"
        ),
        &manager_ref
    ));
    assert!(runtime
        .message_preflight_with_readiness(
            &reply_input,
            &reply_call,
            &specialist_caller,
            |_| panic!("unauthorized reply must not probe readiness"),
        )
        .is_err());
    policy.action_permissions.clear();
    runtime
        .configure_conversation_contexts(Some(policy))
        .unwrap();
    // Prior participants retain an unavailable identity in an existing thread.
    let retained = coworkers(&manager_ref)
        .into_iter()
        .find(|coworker| coworker.handle == "@specialist")
        .unwrap();
    assert_eq!(retained.details["available"], false);
    assert!(runtime
        .message_preflight_with_readiness(&input, &call, &manager, |_| panic!(
            "revoked message must not probe readiness"
        ))
        .is_err());
}
