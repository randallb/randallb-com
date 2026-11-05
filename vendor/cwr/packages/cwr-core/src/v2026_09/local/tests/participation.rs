fn participation_runtime() -> (tempfile::TempDir, LocalRuntime) {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    (temporary, runtime)
}

fn participation_ref(runtime: &LocalRuntime, slug: &str) -> String {
    runtime
        .graph()
        .unwrap()
        .actors
        .iter()
        .find(|actor| actor.coworker_slug == slug)
        .unwrap()
        .actor_ref
        .clone()
}

fn add_participation_actor(runtime: &LocalRuntime, name: &str) -> String {
    let mut state = runtime.lock().unwrap();
    let mut actor = state
        .graph
        .actors
        .iter()
        .find(|actor| actor.coworker_slug == "helper")
        .unwrap()
        .clone();
    actor.actor_ref = format!("cwp://org.example/coworkers/{name}");
    actor.coworker_ref = actor.actor_ref.clone();
    actor.actor_name = Some(name.into());
    actor.coworker_slug = name.into();
    actor.native_tools.clear();
    let target = actor.actor_ref.clone();
    let original_binding = actor.default_actor_integration_ref.clone().unwrap();
    let binding_ref = format!("{target}/bindings/main");
    let integration_ref = format!("{target}/integrations/main");
    actor.default_actor_integration_ref = Some(binding_ref.clone());
    let mut binding = state
        .graph
        .actor_integrations
        .iter()
        .find(|binding| binding.actor_integration_ref == original_binding)
        .unwrap()
        .clone();
    binding.actor_ref = target.clone();
    binding.coworker_ref = target.clone();
    binding.actor_integration_ref = binding_ref.clone();
    binding.integration_ref = integration_ref.clone();
    let mut route = state.graph.routes[0].clone();
    route.actor_ref = target.clone();
    route.coworker_ref = target.clone();
    route.role_ref = actor.role_refs[0].clone();
    route.actor_integration_ref = binding_ref.clone();
    route.action_name = "draft--brief".into();
    route.action_title = "Draft brief".into();
    route.action_description = "Draft a brief".into();
    route.input_schema = json!({"type":"object"});
    route.output_schema = json!({"type":"object"});
    route.adapter_ref = binding.adapter_ref.clone();
    route.integration_ref = binding.integration_ref.clone();
    route.adapter_params = binding.adapter_params.clone();
    route.integration_params = binding.integration_params.clone();
    route.actor_integration_params = binding.actor_integration_params.clone();
    route.coworker_source = binding.coworker_source.clone();
    route.working_path = binding.working_path.clone();
    let routes = vec![route];
    state.graph.actors.push(actor);
    state.graph.actor_integrations.push(binding);
    state.graph.routes.extend(routes);
    state.catalog = local_catalog(&state.graph, &state.host_identity.acting_actor_ref).unwrap();
    target
}

fn mark_actor_ready_for_test(runtime: &LocalRuntime, actor_ref: &str) {
    let mut state = runtime.lock().unwrap();
    state
        .graph
        .actor_integrations
        .iter_mut()
        .filter(|binding| binding.actor_ref == actor_ref)
        .for_each(|binding| binding.adapter_ref = INTRINSIC_CWR_ADAPTER_REF.into());
    state
        .graph
        .routes
        .iter_mut()
        .filter(|route| route.actor_ref == actor_ref)
        .for_each(|route| route.adapter_ref = INTRINSIC_CWR_ADAPTER_REF.into());
}

include!("participation_discovery.rs");

fn claimed_participation_parent(runtime: &LocalRuntime) -> ActionInvocationRecord {
    let request = runtime.admit_text(None, "@helper help").unwrap();
    let mut state = runtime.lock().unwrap();
    match state
        .admission
        .claim_invocation(request.invocation_ref().unwrap(), current_unix_ms())
        .unwrap()
    {
        ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
        _ => panic!("parent was not claimed"),
    }
}

#[test]
fn exact_targets_reject_contract_changes_revocations_and_other_loops() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    let caller = claimed_participation_parent(&runtime);
    let mut state = runtime.lock().unwrap();
    let index = state
        .graph
        .routes
        .iter()
        .position(|route| route.actor_ref == reviewer)
        .unwrap();
    let target = exact_action_target(&state, &state.graph.routes[index]);
    assert!(resolve_exact_action(&state, &caller, &target, None).is_ok());
    let mut other = target.clone();
    other.runtime_ref = "cwp://org.example/loops/other".into();
    assert!(resolve_exact_action(&state, &caller, &other, None)
        .unwrap_err()
        .to_string()
        .contains("cross-runtime"));
    state.graph.routes[index].input_schema = json!({"type":"object","required":["max_width"]});
    assert!(resolve_exact_action(&state, &caller, &target, None)
        .unwrap_err()
        .to_string()
        .contains("contract changed"));
    let fresh = exact_action_target(&state, &state.graph.routes[index]);
    assert!(resolve_exact_action(&state, &caller, &fresh, None).is_ok());
    state
        .graph
        .actor_integrations
        .iter_mut()
        .find(|binding| {
            Some(&binding.actor_integration_ref) == caller.actor_integration_ref.as_ref()
        })
        .unwrap()
        .actor_integration_params["cwr_action_actor_refs"] = json!([]);
    assert!(resolve_exact_action(&state, &caller, &fresh, None)
        .unwrap_err()
        .to_string()
        .contains("not authorized"));
    assert_eq!(state.admission.invocation_records().count(), 1);
}

#[test]
fn exact_binding_survives_default_changes_without_fallback() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    let caller = claimed_participation_parent(&runtime);
    let mut state = runtime.lock().unwrap();
    let original = state
        .graph
        .routes
        .iter()
        .find(|route| route.actor_ref == reviewer)
        .unwrap()
        .clone();
    let target = exact_action_target(&state, &original);
    let mut second = state
        .graph
        .actor_integrations
        .iter()
        .find(|binding| binding.actor_ref == reviewer)
        .unwrap()
        .clone();
    second.actor_integration_ref = format!("{reviewer}/bindings/second");
    let second_ref = second.actor_integration_ref.clone();
    state.graph.actor_integrations.push(second);
    let mut second_route = original.clone();
    second_route.actor_integration_ref = second_ref.clone();
    state.graph.routes.push(second_route);
    state
        .graph
        .actors
        .iter_mut()
        .find(|actor| actor.actor_ref == reviewer)
        .unwrap()
        .default_actor_integration_ref = Some(second_ref.clone());
    let catalog = local_catalog(&state.graph, &caller.target_actor_ref).unwrap();
    assert_eq!(
        catalog
            .actor_by_ref(&reviewer)
            .unwrap()
            .actor_integration_ref,
        second_ref
    );
    assert_eq!(
        resolve_exact_action(&state, &caller, &target, None)
            .unwrap()
            .actor_integration_ref,
        original.actor_integration_ref
    );
}

#[test]
fn cross_runtime_calls_preserve_target_and_start_no_work_or_membership() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    let caller = claimed_participation_parent(&runtime);
    let (mut target, before) = {
        let state = runtime.lock().unwrap();
        let route = state
            .graph
            .routes
            .iter()
            .find(|route| route.actor_ref == reviewer)
            .unwrap();
        (
            exact_action_target(&state, route),
            state
                .admission
                .conversation(caller.conversation_ref.as_deref().unwrap())
                .unwrap()
                .clone(),
        )
    };
    target.runtime_ref = "cwp://org.example/loops/another-configuration-of-the-same-source".into();
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    for (operation, input) in [
        (
            INVOKE_ACTION_HOST_OPERATION,
            json!({"target":target,"input":{}}),
        ),
        (
            SEND_MESSAGE_HOST_OPERATION,
            json!({"runtime_ref":target.runtime_ref,"to":reviewer,"text":"help"}),
        ),
    ] {
        let call = AdapterCall {
            invocation_ref: caller.invocation_ref.clone(),
            call_id: operation.into(),
            operation: operation.into(),
            input,
        };
        let error = runtime
            .handle_host_call(
                call,
                &caller,
                &queue,
                &AdapterModelOutputMailbox::discard(),
                &mut |_| {},
            )
            .unwrap_err();
        assert!(error.to_string().contains("cross-runtime"));
    }
    let state = runtime.lock().unwrap();
    assert_eq!(state.admission.invocation_records().count(), 1);
    assert_eq!(
        state
            .admission
            .conversation(caller.conversation_ref.as_deref().unwrap())
            .unwrap(),
        &before
    );
    assert!(queue.borrow().is_empty());
}

#[test]
fn message_binding_rejections_do_not_admit_a_child_or_change_the_shared_thread() {
    for host_only_target in [false, true] {
        let (_temporary, runtime) = participation_runtime();
        let reviewer = add_participation_actor(&runtime, "reviewer");
        let caller = claimed_participation_parent(&runtime);
        let conversation_ref = caller.conversation_ref.as_deref().unwrap();
        let before = runtime.conversation(conversation_ref).unwrap().unwrap();
        let target = if host_only_target {
            participation_ref(&runtime, "person")
        } else {
            let mut state = runtime.lock().unwrap();
            state
                .graph
                .actor_integrations
                .iter_mut()
                .find(|binding| binding.actor_ref == reviewer)
                .unwrap()
                .adapter_ref = "cwp://org.example/adapters/unimplemented".into();
            reviewer
        };
        let mut input = json!({"to":target,"text":"Please help"});
        if host_only_target {
            input["binding_ref"] = json!("cwp://org.example/bindings/missing");
        }
        let call = AdapterCall {
            invocation_ref: caller.invocation_ref.clone(),
            call_id: "rejected-message".into(),
            operation: SEND_MESSAGE_HOST_OPERATION.into(),
            input,
        };
        let result = runtime.handle_send_message(
            &call,
            &caller,
            &Rc::new(RefCell::new(VecDeque::new())),
            &AdapterModelOutputMailbox::discard(),
            &mut |_| {},
        );
        let Err(error) = result else {
            panic!("unavailable selected binding was accepted")
        };
        assert!(error.to_string().contains(if host_only_target {
            "explicit message binding"
        } else {
            "not available in this version of Workloop"
        }));
        assert_eq!(
            runtime
                .lock()
                .unwrap()
                .admission
                .invocation_records()
                .count(),
            1
        );
        let after = runtime.conversation(conversation_ref).unwrap().unwrap();
        assert_eq!(after.participant_actor_refs, before.participant_actor_refs);
        assert_eq!(after.messages, before.messages);
    }
}

#[test]
fn queued_child_and_call_retry_recheck_current_request_permissions() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    let caller = claimed_participation_parent(&runtime);
    let target = {
        let state = runtime.lock().unwrap();
        exact_action_target(
            &state,
            state
                .graph
                .routes
                .iter()
                .find(|route| route.actor_ref == reviewer)
                .unwrap(),
        )
    };
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "queued-action".into(),
        operation: INVOKE_ACTION_HOST_OPERATION.into(),
        input: json!({"target": target, "input": {}}),
    };
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    runtime.handle_start_action(&call, &caller, &queue).unwrap();
    let child_ref = queue.borrow_mut().pop_front().unwrap();
    let before = runtime
        .conversation(caller.conversation_ref.as_deref().unwrap())
        .unwrap()
        .unwrap();
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
    assert!(runtime
        .handle_start_action(&call, &caller, &queue)
        .unwrap_err()
        .to_string()
        .contains("no longer authorized"));
    let claim = runtime.claim_execution(&child_ref, &ProcessAdapterStopControl::new());
    let Err(error) = claim else {
        panic!("revoked queued child was claimed")
    };
    assert!(error.to_string().contains("no longer authorized"));
    let state = runtime.lock().unwrap();
    assert!(
        !state
            .admission
            .invocation_record(&child_ref)
            .unwrap()
            .attempt_active
    );
    assert!(!state.active.contains_key(&child_ref));
    assert_eq!(state.admission.invocation_records().count(), 2);
    assert_eq!(
        state
            .admission
            .conversation(caller.conversation_ref.as_deref().unwrap())
            .unwrap(),
        &before
    );
    assert!(queue.borrow().is_empty());
}

#[test]
fn message_reload_requires_both_workspace_capability_and_current_coworker_grant() {
    let (_temporary, runtime) = participation_runtime();
    let caller = claimed_participation_parent(&runtime);
    let mut record = caller.clone();
    record.parent_invocation_ref = Some(caller.invocation_ref.clone());
    record.requester_actor_ref = caller.target_actor_ref.clone();
    let invocation = {
        let state = runtime.lock().unwrap();
        state
            .graph
            .actor_integrations
            .iter()
            .find(|binding| binding.actor_ref == INTRINSIC_CWR_ACTOR_REF)
            .unwrap()
            .project_adapter_invocation(
                "cwp://org.example/invocations/message-reload",
                RESPOND_TO_MESSAGE_ACTION_NAME,
                json!({"source_message_ref":"message-reload"}),
                json!({"type":"object"}),
                json!({"messages":[{"message_ref":"message-reload","text":"reload graph"}]}),
            )
    };
    // The control command uses the same path regardless of message versus named-action delivery.
    for (workspace, grant) in [(false, false), (true, false), (false, true)] {
        let mut state = runtime.lock().unwrap();
        state.allow_reconfiguration = workspace;
        let actor = state
            .graph
            .actors
            .iter_mut()
            .find(|actor| actor.actor_ref == caller.target_actor_ref)
            .unwrap();
        actor.authority = if grant {
            vec!["cwr:configure".into()]
        } else {
            vec![]
        };
        drop(state);
        let error = runtime
            .execute_intrinsic_invocation(&record, &invocation)
            .unwrap_err();
        assert!(
            error.to_string().contains("configuration capability"),
            "{error}"
        );
    }
}

#[test]
fn nested_rename_targets_the_requesting_conversation_not_private_execution() {
    let (_temporary, runtime) = participation_runtime();
    let caller = claimed_participation_parent(&runtime);
    let requesting_conversation_ref = caller.conversation_ref.clone().unwrap();
    let target = {
        let state = runtime.lock().unwrap();
        let route = state
            .graph
            .routes
            .iter()
            .find(|route| route.action_name == "rename--conversation")
            .unwrap();
        exact_action_target(&state, route)
    };
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "rename-requesting-conversation".into(),
        operation: INVOKE_ACTION_HOST_OPERATION.into(),
        input: json!({
            "target": target,
            "input": {"title": "Nested title"}
        }),
    };
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    runtime.handle_start_action(&call, &caller, &queue).unwrap();
    let child_ref = queue.borrow_mut().pop_front().unwrap();
    let execution_conversation_ref = runtime
        .lock()
        .unwrap()
        .admission
        .execution_conversation_ref(&child_ref)
        .unwrap()
        .to_owned();
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(task.record().invocation_ref, child_ref);
    task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
        .unwrap();

    assert_eq!(
        runtime
            .conversation(&requesting_conversation_ref)
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("Nested title")
    );
    assert_eq!(
        runtime
            .conversation(&execution_conversation_ref)
            .unwrap()
            .unwrap()
            .title,
        None
    );
    let execution = runtime
        .conversation(&execution_conversation_ref)
        .unwrap()
        .unwrap();
    assert_eq!(
        execution.parent_conversation_ref.as_deref(),
        Some(requesting_conversation_ref.as_str())
    );
    assert!(!execution.is_sidebar_eligible());
    let parent = runtime
        .host_conversation(&requesting_conversation_ref)
        .unwrap()
        .unwrap();
    assert!(parent.is_sidebar_eligible());
    assert!(parent.messages.iter().any(|message| matches!(
        &message.action,
        Some(crate::v2026_09::conversation::ConversationActionEvent::Result { .. })
    )));
    assert!(parent.activity.iter().any(|activity| matches!(
        &activity.event, ConversationActivityEvent::WorkLifecycle { invocation_ref, parent_invocation_ref: Some(parent), .. }
            if invocation_ref == &child_ref && parent == &caller.invocation_ref
    )));
}

#[test]
fn experimental_feature_discovery_requires_reconfiguration_authority() {
    let (_temporary, runtime) = participation_runtime();
    let caller = claimed_participation_parent(&runtime);
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "feature-discovery".into(),
        operation: LIST_ACTIONS_HOST_OPERATION.into(),
        input: json!({}),
    };
    for (workspace, grant) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut state = runtime.lock().unwrap();
        state.allow_reconfiguration = workspace;
        let actor = state
            .graph
            .actors
            .iter_mut()
            .find(|actor| actor.actor_ref == caller.target_actor_ref)
            .unwrap();
        actor.authority = if grant {
            vec!["cwr:configure".into()]
        } else {
            vec![]
        };
        drop(state);
        let AdapterHostCallOutcome::Returned(result) =
            runtime.handle_discovery_call(&call, &caller).unwrap();
        for activation in [
            "@cwr--read--experimental_features",
            "@cwr--set--experimental_features",
        ] {
            assert_eq!(
                result["actions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|action| action["activation"] == activation),
                workspace && grant
            );
        }
    }
}

include!("participation_preferences.rs");
