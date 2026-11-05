#[test]
fn discovery_lists_loaded_coworkers_without_adding_participants() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    mark_actor_ready_for_test(&runtime, &reviewer);
    let person = participation_ref(&runtime, "person");
    let admission = runtime.admit_text(None, "hello").unwrap();
    assert_eq!(admission.conversation.participant_actor_refs, vec![person.clone()]);
    let Some(IntrinsicCwrResponse::Coworkers { coworkers }) = runtime.discovery_as(
        Some(admission.conversation_ref()), &person, None, &IntrinsicCwrCommand::ListCoworkers,
    ).unwrap() else { panic!("missing directory") };
    let found = coworkers.iter().find(|coworker| coworker.handle == "@reviewer").unwrap();
    assert_eq!(found.details["coworker_ref"], reviewer);
    assert_eq!(found.details["participant"], false);
    assert_eq!(found.details["bindings"][0]["available"], true);
    assert_eq!(found.details["bindings"][0]["readiness"]["status"], "ready");
    assert_eq!(runtime.conversation(admission.conversation_ref()).unwrap().unwrap().participant_actor_refs, vec![person]);
    assert!(runtime.lock().unwrap().admission.invocation_records().next().is_none());
}

#[test]
fn discovery_explains_an_unavailable_adapter_binding() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    let person = participation_ref(&runtime, "person");
    let admission = runtime.admit_text(None, "hello").unwrap();
    let integration_ref = "cwp://org.example/integrations/unimplemented".to_owned();
    let adapter_ref = "cwp://org.example/adapters/unimplemented".to_owned();
    let mut state = runtime.lock().unwrap();
    {
        let binding = state.graph.actor_integrations.iter_mut().find(|binding| binding.actor_ref == reviewer).unwrap();
        binding.integration_ref = integration_ref.clone();
        binding.adapter_ref = adapter_ref.clone();
    }
    for route in state.graph.routes.iter_mut().filter(|route| route.actor_ref == reviewer) {
        route.integration_ref = integration_ref.clone();
        route.adapter_ref = adapter_ref.clone();
    }
    drop(state);
    let Some(IntrinsicCwrResponse::Coworkers { coworkers }) = runtime.discovery_as(
        Some(admission.conversation_ref()), &person, None, &IntrinsicCwrCommand::ListCoworkers,
    ).unwrap() else { panic!("missing directory") };
    let found = coworkers.iter().find(|coworker| coworker.handle == "@reviewer").unwrap();
    assert_eq!(found.details["bindings"][0]["available"], false);
    assert_eq!(found.details["bindings"][0]["readiness"]["code"], "adapter_implementation_missing");
    assert_eq!(found.details["bindings"][0]["readiness"]["message"], "This coworker is not available in this version of Workloop.");
}

#[test]
fn accepted_request_adds_only_the_requested_coworker() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    add_participation_actor(&runtime, "ops");
    let person = participation_ref(&runtime, "person");
    let initial = runtime.admit_text(None, "hello").unwrap();
    let request = runtime.admit_text(Some(initial.conversation_ref()), "@reviewer review this").unwrap();
    assert_eq!(request.conversation.participant_actor_refs, vec![person, reviewer]);
}

#[test]
fn nonparticipant_actions_are_discoverable_with_exact_targets() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    mark_actor_ready_for_test(&runtime, &reviewer);
    let person = participation_ref(&runtime, "person");
    let initial = runtime.admit_text(None, "hello").unwrap();
    let Some(IntrinsicCwrResponse::Actions { targets, .. }) = runtime.discovery_as(
        Some(initial.conversation_ref()), &person, None, &IntrinsicCwrCommand::ListActions,
    ).unwrap() else { panic!("missing actions") };
    let target = targets.get("@reviewer--draft--brief").unwrap();
    assert_eq!(target.coworker_ref, reviewer);
    assert_eq!(target.runtime_ref, runtime.graph().unwrap().runtime_ref);
    assert!(target.contract_id.starts_with("sha256:"));
    assert!(!targets.contains_key("@cwr--invite--coworker"));
}

fn list_actions_for(
    runtime: &LocalRuntime,
    caller: &ActionInvocationRecord,
    input: Value,
) -> CwrResult<Value> {
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "coworker-action-discovery".into(),
        operation: LIST_ACTIONS_HOST_OPERATION.into(),
        input,
    };
    let AdapterHostCallOutcome::Returned(result) = runtime.handle_discovery_call(&call, caller)?;
    Ok(result)
}

#[test]
fn action_discovery_accepts_handles_and_full_references() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    mark_actor_ready_for_test(&runtime, &reviewer);
    let caller = claimed_participation_parent(&runtime);

    let by_ref = list_actions_for(&runtime, &caller, json!({"coworker_ref": reviewer})).unwrap();
    let by_handle =
        list_actions_for(&runtime, &caller, json!({"coworker_handle": "@reviewer"})).unwrap();

    assert_eq!(by_handle, by_ref);
    assert_eq!(by_handle["actions"][0]["activation"], "@reviewer--draft--brief");
    assert_eq!(by_handle["actions"][0]["target"]["coworker_ref"], reviewer);
    assert!(
        list_actions_for(&runtime, &caller, json!({"coworker_ref": "@reviewer"}))
            .unwrap_err()
            .to_string()
            .contains("unknown discovery coworker reference @reviewer")
    );
}

#[test]
fn action_discovery_explains_unknown_coworkers() {
    let (_temporary, runtime) = participation_runtime();
    let caller = claimed_participation_parent(&runtime);

    let error = list_actions_for(&runtime, &caller, json!({"coworker_handle": "@missing"}))
        .unwrap_err()
        .to_string();

    assert!(error.contains("unknown discovery coworker handle @missing"));
    assert!(error.contains("call list--coworkers"));
}

#[test]
fn action_discovery_rejects_ambiguous_handles_without_selecting_one() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    let duplicate = add_participation_actor(&runtime, "duplicate");
    runtime
        .lock()
        .unwrap()
        .graph
        .actors
        .iter_mut()
        .find(|actor| actor.actor_ref == duplicate)
        .unwrap()
        .coworker_slug = "reviewer".into();
    let caller = claimed_participation_parent(&runtime);

    let error = list_actions_for(&runtime, &caller, json!({"coworker_handle": "@reviewer"}))
        .unwrap_err()
        .to_string();

    assert!(error.contains("handle @reviewer is ambiguous"));
    assert!(error.contains(&reviewer));
    assert!(error.contains(&duplicate));
    assert!(error.contains("exact coworker_ref"));
    assert!(list_actions_for(&runtime, &caller, json!({"coworker_ref": reviewer})).is_ok());
}

#[test]
fn action_discovery_rejects_multiple_coworker_selectors() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    let caller = claimed_participation_parent(&runtime);

    let error = list_actions_for(
        &runtime,
        &caller,
        json!({"coworker_ref": reviewer, "coworker_handle": "@reviewer"}),
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("either coworker_ref or coworker_handle, not both"));
}
