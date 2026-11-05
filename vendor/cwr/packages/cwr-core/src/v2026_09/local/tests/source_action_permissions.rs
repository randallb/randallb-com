#[test]
fn source_action_permissions_scope_both_hops_and_revalidate_retained_results() {
    use crate::v2026_09::upstream::{ContextActionPermission, ConversationContextPolicy};
    let (_temporary, runtime) = participation_runtime();
    let source = add_participation_actor(&runtime, "groupme");
    let native = add_participation_actor(&runtime, "native_ops");
    let foreign = add_participation_actor(&runtime, "foreign");
    for actor in [&source, &native, &foreign] {
        mark_actor_ready_for_test(&runtime, actor);
    }
    let family = claimed_participation_parent(&runtime);
    {
        let mut state = runtime.lock().unwrap();
        let binding = state
            .graph
            .actor_integrations
            .iter_mut()
            .find(|binding| binding.actor_ref == family.target_actor_ref)
            .unwrap();
        binding.actor_integration_params["cwr_action_actor_refs"] = json!([source]);
        let binding = state
            .graph
            .actor_integrations
            .iter_mut()
            .find(|binding| binding.actor_ref == foreign)
            .unwrap();
        binding
            .actor_integration_params
            .as_object_mut()
            .unwrap()
            .remove("cwr_action_actor_refs");
        let mut forbidden = state
            .graph
            .routes
            .iter()
            .find(|route| route.actor_ref == native)
            .unwrap()
            .clone();
        forbidden.action_name = "write--settings".into();
        state.graph.routes.push(forbidden);
    }
    let mut policy = ConversationContextPolicy {
        suspended: false,
        default_context_ref: "family".into(),
        contexts: BTreeMap::from([
            (
                "family".into(),
                vec![
                    family.target_actor_ref.clone(),
                    foreign.clone(),
                    native.clone(),
                ],
            ),
            ("unrelated".into(), vec![foreign.clone()]),
        ]),
        action_permissions: vec![
            ContextActionPermission {
                caller_actor_ref: family.target_actor_ref.clone(),
                target_actor_ref: source.clone(),
                action_names: vec!["draft--brief".into()],
            },
            ContextActionPermission {
                caller_actor_ref: source.clone(),
                target_actor_ref: native.clone(),
                action_names: vec!["draft--brief".into()],
            },
        ],
    };
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    assert!(!runtime
        .conversation_context_targets(Some("family"))
        .unwrap()
        .unwrap()
        .contains(&source));
    let found = list_actions_for(&runtime, &family, json!({"coworker_ref":source})).unwrap();
    let source_target = found["actions"][0]["target"].clone();
    assert_eq!(source_target["coworker_ref"], source);
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    let call = |caller: &ActionInvocationRecord, id: &str, target: Value| AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: id.into(),
        operation: INVOKE_ACTION_HOST_OPERATION.into(),
        input: json!({"target":target,"input":{}}),
    };
    let first = call(&family, "family-source", source_target.clone());
    runtime
        .handle_start_action(&first, &family, &queue)
        .unwrap();
    let source_ref = queue.borrow_mut().pop_front().unwrap();
    let source_caller = claim_source_action_for_test(&runtime, &source_ref);
    let native_actions =
        list_actions_for(&runtime, &source_caller, json!({"coworker_ref":native})).unwrap();
    assert_eq!(native_actions["actions"].as_array().unwrap().len(), 1);
    assert_eq!(
        native_actions["actions"][0]["target"]["action_name"],
        "draft--brief"
    );
    let second = call(
        &source_caller,
        "source-native",
        native_actions["actions"][0]["target"].clone(),
    );
    runtime
        .handle_start_action(&second, &source_caller, &queue)
        .unwrap();
    let native_ref = queue.borrow_mut().pop_front().unwrap();
    let native_record = runtime
        .lock()
        .unwrap()
        .admission
        .invocation_record(&native_ref)
        .unwrap()
        .clone();
    {
        let state = runtime.lock().unwrap();
        require_native_gateway_result_current(&state, &native_record).unwrap();
        let forbidden = state
            .graph
            .routes
            .iter()
            .find(|route| route.actor_ref == native && route.action_name == "write--settings")
            .unwrap();
        let attempt = call(
            &source_caller,
            "forbidden-native",
            serde_json::to_value(exact_action_target(&state, forbidden)).unwrap(),
        );
        drop(state);
        assert!(runtime
            .handle_start_action(&attempt, &source_caller, &queue)
            .is_err());
    }
    // A collaborator with no actor allowlist must not discover or invoke the source.
    let person = participation_ref(&runtime, "person");
    let foreign_request = runtime
        .submit_message_in_context(
            "cwp://example.test/foreign",
            "foreign",
            &person,
            None,
            "@foreign test",
            vec![],
            None,
            Some("family"),
        )
        .unwrap();
    let foreign_caller =
        claim_source_action_for_test(&runtime, foreign_request.invocation_ref().unwrap());
    assert!(action_actor_allowlist(
        &runtime.lock().unwrap().graph,
        &foreign,
        foreign_caller.actor_integration_ref.as_deref()
    )
    .unwrap()
    .is_none());
    assert!(list_actions_for(&runtime, &foreign_caller, json!({"coworker_ref":source})).is_err());
    assert!(runtime
        .handle_start_action(
            &call(&foreign_caller, "foreign-source", source_target),
            &foreign_caller,
            &queue
        )
        .is_err());
    // Either hop can be revoked independently; retained results require both.
    for removed in 0..2 {
        let mut one_hop = policy.clone();
        one_hop.action_permissions.remove(removed);
        runtime
            .configure_conversation_contexts(Some(one_hop))
            .unwrap();
        let state = runtime.lock().unwrap();
        assert!(require_native_gateway_result_current(&state, &native_record).is_err());
    }
    // Cached exact targets and successful protected results lose permission together.
    policy.action_permissions.clear();
    runtime
        .configure_conversation_contexts(Some(policy))
        .unwrap();
    assert!(runtime
        .handle_start_action(&second, &source_caller, &queue)
        .is_err());
    let state = runtime.lock().unwrap();
    assert!(state
        .admission
        .check_context_execution(&state.graph, &native_record)
        .is_err());
    assert!(require_native_gateway_result_current(&state, &native_record).is_err());
}

fn claim_source_action_for_test(
    runtime: &LocalRuntime,
    invocation: &str,
) -> ActionInvocationRecord {
    let mut state = runtime.lock().unwrap();
    match state
        .admission
        .claim_invocation(invocation, current_unix_ms())
        .unwrap()
    {
        ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
        _ => panic!("source action not claimed"),
    }
}
