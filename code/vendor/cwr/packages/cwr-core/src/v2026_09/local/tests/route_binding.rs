#[test]
fn queued_invocations_keep_their_admitted_execution_route_after_activation() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "@helper draft this").unwrap();
    let state = runtime.lock().unwrap();
    let record = state
        .admission
        .invocation_record(admission.invocation_ref().unwrap())
        .unwrap();
    let invocation = state
        .admission
        .resume_admitted(&admission.record)
        .unwrap()
        .unwrap()
        .invocation
        .unwrap();
    selected_process_route(&state.graph, record, &mut invocation.clone()).unwrap();
    for change in 0..6 {
        let mut graph = state.graph.clone();
        let binding = graph
            .actor_integrations
            .iter_mut()
            .find(|binding| {
                Some(&binding.actor_integration_ref) == record.actor_integration_ref.as_ref()
            })
            .unwrap();
        match change {
            0 => binding.working_path = temporary.path().join("different-workspace"),
            1 => binding.adapter_ref = "cwp://org.coworkerprotocol/2026-09/adapters/sh".into(),
            2 => binding.actor_integration_params["command"] = json!("different-command"),
            3 => binding.integration_params["endpoint"] = json!("different-endpoint"),
            4 => binding.available_authority.push("changed-authority".into()),
            _ => {
                binding.coworker_source = temporary.path().join("different-coworker/coworker.toml")
            }
        }
        let fresh = binding.project_adapter_invocation(
            "cwp://example.test/invocations/fresh",
            &invocation.action_name,
            invocation.input.clone(),
            invocation.output_schema.clone(),
            invocation.thread.clone(),
        );
        let selected = selected_process_route(&graph, record, &mut invocation.clone()).unwrap();
        assert_eq!(selected.fingerprint(), invocation.route_fingerprint.clone().unwrap(), "change {change}");
        assert!(selected_process_route(&graph, record, &mut fresh.clone()).is_ok());
    }
    let mut legacy = invocation;
    legacy.route_fingerprint = None;
    assert!(selected_process_route(&state.graph, record, &mut legacy)
        .unwrap_err()
        .to_string()
        .contains("does not match"));
}

#[test]
fn queued_native_tools_keep_their_admitted_targets_and_schemas_after_activation() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    let helper = participation_ref(&runtime, "helper");
    let original = {
        let mut state = runtime.lock().unwrap();
        let route = state.graph.routes.iter().find(|route| route.actor_ref == reviewer).unwrap().clone();
        state.graph.actors.iter_mut().find(|actor| actor.actor_ref == helper).unwrap().native_tools = vec![
            crate::v2026_09::graph::ResolvedNativeTool {
                coworker_ref: reviewer.clone(), action_name: route.action_name.clone(),
                binding_ref: Some(route.actor_integration_ref.clone()),
            }
        ];
        retain_native_tool_definitions(&mut state.graph).unwrap();
        exact_action_target(&state, &route)
    };
    let admission = runtime.admit_text(None, "@helper draft this").unwrap();
    let (mut admitted, record, mut fresh, replacement) = {
        let mut state = runtime.lock().unwrap();
        let admitted = state.admission.resume_admitted(&admission.record).unwrap().unwrap().invocation.unwrap();
        let record = state.admission.invocation_record(admission.invocation_ref().unwrap()).unwrap().clone();
        let route = state.graph.routes.iter_mut().find(|route| route.actor_ref == reviewer).unwrap();
        route.input_schema = json!({"type":"object","properties":{"revised":{"type":"boolean"}},"required":["revised"]});
        route.contract_revision = "replacement".into();
        retain_native_tool_definitions(&mut state.graph).unwrap();
        let replacement = exact_action_target(&state, state.graph.routes.iter().find(|route| route.actor_ref == reviewer).unwrap());
        let binding = state.graph.actor_integrations.iter().find(|binding|
            Some(&binding.actor_integration_ref) == record.actor_integration_ref.as_ref()).unwrap();
        let fresh = binding.project_adapter_invocation("cwp://example.test/invocations/fresh",
            &admitted.action_name, admitted.input.clone(), admitted.output_schema.clone(), admitted.thread.clone());
        (admitted, record, fresh, replacement)
    };
    assert_ne!(original, replacement);
    assert_ne!(admitted.route_fingerprint, fresh.route_fingerprint);
    assert_eq!(admitted.provider_fingerprint, fresh.provider_fingerprint);
    runtime.install_native_actions(&mut admitted, &record, &BTreeMap::new()).unwrap();
    runtime.install_native_actions(&mut fresh, &record, &BTreeMap::new()).unwrap();
    let old = admitted.host_operations.iter().find(|operation| operation.name == "cwp.reviewer--draft--brief").unwrap();
    let new = fresh.host_operations.iter().find(|operation| operation.name == old.name).unwrap();
    assert_eq!(old.input_schema["x-cwr-target"], serde_json::to_value(original).unwrap());
    assert_eq!(new.input_schema["x-cwr-target"], serde_json::to_value(replacement).unwrap());
    assert!(old.input_schema.get("required").is_none());
    assert_eq!(new.input_schema["required"], json!(["revised"]));
}

#[test]
fn queued_named_actions_bind_shared_context_at_first_execution_and_keep_it_on_retry() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    let caller = claimed_participation_parent(&runtime);
    let target = {
        let state = runtime.lock().unwrap();
        exact_action_target(&state, state.graph.routes.iter().find(|route| route.actor_ref == reviewer).unwrap())
    };
    let call = AdapterCall { invocation_ref: caller.invocation_ref.clone(), call_id:"queued-context".into(),
        operation:INVOKE_ACTION_HOST_OPERATION.into(), input:json!({"target":target,"input":{}}) };
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    runtime.handle_start_action(&call, &caller, &queue).unwrap();
    let child_ref = queue.borrow_mut().pop_front().unwrap();
    let mut state = runtime.lock().unwrap();
    state.admission.record_invocation_progress_with_publication(&caller.invocation_ref, 102,
        "Update while named action waits", true).unwrap();
    let ConversationInvocationClaimOutcome::Claimed(first) = state.admission.claim_invocation(&child_ref, 100).unwrap()
        else { panic!("named child was not claimed") };
    assert!(!first.adapter_invocation.conversation_continuity);
    assert!(first.adapter_invocation.thread["messages"].as_array().unwrap().iter()
        .any(|message| message["text"] == "Update while named action waits"));
    assert_eq!(first.adapter_invocation.thread["context_initialized"], true);
    state.admission.recover_interrupted_invocation(&child_ref).unwrap();
    state.admission.record_invocation_progress_with_publication(&caller.invocation_ref, 102,
        "Later update after execution began", true).unwrap();
    let ConversationInvocationClaimOutcome::Claimed(retry) = state.admission.claim_invocation(&child_ref, 101).unwrap()
        else { panic!("named child was not reclaimed") };
    assert_eq!(first.adapter_invocation.thread, retry.adapter_invocation.thread);
}
