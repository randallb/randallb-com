#[test]
fn schedules_spike_native_actions_use_requesting_conversation_owner() {
    let (_temporary, runtime, _, a, b) = installed_schedule_fixture();
    let person = participation_ref(&runtime, "person");
    let list = runtime
        .submit_message_in_context(
            "cwp://example.test/control",
            "list-a",
            &person,
            None,
            "@cwr--list--schedules",
            vec![],
            None,
            Some(&a),
        )
        .unwrap();
    let turn = runtime.execute_admission(&list, |_| {}).unwrap();
    let invocation = turn.invocation.unwrap();
    assert_eq!(invocation.state, ActionInvocationState::Completed);
    let result = invocation
        .terminal_response()
        .unwrap()
        .result
        .as_ref()
        .unwrap();
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["owner_context_ref"], a);
    let own_ref = result[0]["schedule_ref"].as_str().unwrap();
    let foreign = runtime
        .schedules_module_as(&b, &person)
        .unwrap()
        .list()
        .unwrap()
        .pop()
        .unwrap();
    let request = runtime
        .admit_action_as(
            "cwp://example.test/control",
            "pause-foreign",
            &person,
            Some(list.conversation_ref()),
            "@cwr--pause--schedule",
            json!({"schedule_ref":foreign.schedule_ref}),
        )
        .unwrap();
    let denied = runtime.execute_admission(&request, |_| {}).unwrap();
    assert_eq!(
        denied.invocation.unwrap().state,
        ActionInvocationState::Failed
    );
    assert!(
        !runtime
            .schedules_module_as(&b, &person)
            .unwrap()
            .get(&foreign.schedule_ref)
            .unwrap()
            .paused
    );
    let pause = runtime
        .admit_action_as(
            "cwp://example.test/control",
            "pause-own",
            &person,
            Some(list.conversation_ref()),
            "@cwr--pause--schedule",
            json!({"schedule_ref":own_ref}),
        )
        .unwrap();
    assert_eq!(
        runtime
            .execute_admission(&pause, |_| {})
            .unwrap()
            .invocation
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );
    assert!(
        runtime
            .schedules_module_as(&a, &person)
            .unwrap()
            .get(own_ref)
            .unwrap()
            .paused
    );
}

#[test]
fn schedule_control_history_exposes_only_public_collection_fields() {
    let (_temporary, runtime, _, owner, _) = installed_schedule_fixture();
    let person = participation_ref(&runtime, "person");
    let schedule = runtime
        .schedules_module_as(&owner, &person)
        .unwrap()
        .list()
        .unwrap()
        .pop()
        .unwrap();
    let empty_runs = runtime
        .admit_owner_schedule_action_in_upstream_scope(
            "cwp://example.test/control",
            "public-empty-runs",
            &person,
            &owner,
            "@cwr--list--schedule_runs",
            json!({"schedule_ref": schedule.schedule_ref}),
            None,
        )
        .unwrap();
    let empty_invocation = runtime
        .execute_admission(&empty_runs, |_| {})
        .unwrap()
        .invocation
        .unwrap();
    let empty_terminal = empty_invocation.terminal_response().unwrap();
    assert_eq!(empty_terminal.result.as_ref().unwrap(), &json!([]));
    assert_eq!(empty_terminal.summary, "No recent runs.");
    runtime.tick_schedules(2000).unwrap();
    for (index, action) in [
        "list--schedules",
        "get--schedule",
        "pause--schedule",
        "resume--schedule",
        "list--schedule_runs",
    ]
    .into_iter()
    .enumerate()
    {
        let input = if action == "list--schedules" {
            json!({})
        } else {
            json!({"schedule_ref": schedule.schedule_ref})
        };
        let admission = runtime
            .admit_owner_schedule_action_in_upstream_scope(
                "cwp://example.test/control",
                format!("public-schedule-{index}"),
                &person,
                &owner,
                &format!("@cwr--{action}"),
                input,
                None,
            )
            .unwrap();
        let invocation = runtime.execute_admission(&admission, |_| {}).unwrap().invocation.unwrap();
        assert_eq!(invocation.state, ActionInvocationState::Completed);
        let terminal = invocation.terminal_response().unwrap();
        let result = terminal.result.as_ref().unwrap();
        let serialized = serde_json::to_string(result).unwrap();
        for retained in [&serialized, &terminal.summary] {
            assert!(!retained.contains("CWR_SCHEDULE_PRIVATE_SENTINEL"), "{action} leaked saved input");
            assert!(!retained.contains("\"target\""), "{action} leaked exact target");
            assert!(!retained.contains("\"input\""), "{action} leaked saved input field");
        }
        if action == "list--schedule_runs" {
            let runs = result.as_array().unwrap();
            assert!(!runs.is_empty());
            assert!(runs.iter().all(|run| run.get("definition").is_none()));
        } else {
            let row = if action == "list--schedules" { &result[0] } else { result };
            assert_eq!(row["schedule_ref"], schedule.schedule_ref);
            assert!(row["definition"]["declaration_ref"].is_string());
            assert!(row["definition"].get("target").is_none());
            assert!(row["definition"].get("input").is_none());
        }
    }
    // Schedule execution still retains the declaration fields it needs.
    let stored = runtime
        .schedules_module_as(&owner, &person)
        .unwrap()
        .get(&schedule.schedule_ref)
        .unwrap();
    assert_eq!(stored.definition.input["private_token"], "CWR_SCHEDULE_PRIVATE_SENTINEL");
}

#[test]
fn owner_destination_schedule_actions_recover_without_retargeting() {
    let (_temporary, runtime, _, a, b) = installed_schedule_fixture();
    let person = participation_ref(&runtime, "person");
    let schedule = runtime
        .schedules_module_as(&a, &person)
        .unwrap()
        .list()
        .unwrap()
        .pop()
        .unwrap();
    let admission = runtime
        .admit_owner_schedule_action_in_upstream_scope(
            "cwp://example.test/control",
            "owner-pause",
            &person,
            &a,
            "@cwr--pause--schedule",
            json!({"schedule_ref": schedule.schedule_ref}),
            None,
        )
        .unwrap();
    assert_eq!(
        admission.conversation.owner_context_ref.as_deref(),
        Some(a.as_str())
    );
    let repeated = runtime
        .admit_owner_schedule_action_in_upstream_scope(
            "cwp://example.test/control",
            "owner-pause",
            &person,
            &a,
            "@cwr--pause--schedule",
            json!({"schedule_ref": schedule.schedule_ref}),
            None,
        )
        .unwrap();
    assert_eq!(admission.invocation_ref(), repeated.invocation_ref());
    assert!(runtime
        .admit_action_in_upstream_scope(
            "cwp://example.test/control",
            "owner-pause",
            &person,
            None,
            "@cwr--pause--schedule",
            json!({"schedule_ref": schedule.schedule_ref}),
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("schedule recovery requires its installed owner destination"));
    assert!(runtime
        .admit_owner_schedule_action_in_upstream_scope(
            "cwp://example.test/control",
            "owner-pause",
            &person,
            &b,
            "@cwr--pause--schedule",
            json!({"schedule_ref": schedule.schedule_ref}),
            None,
        )
        .is_err());
    assert!(runtime
        .admit_owner_schedule_action_in_upstream_scope(
            "cwp://example.test/control",
            "root-pause",
            &person,
            "cwp://example.test/workspace",
            "@cwr--pause--schedule",
            json!({"schedule_ref": schedule.schedule_ref}),
            None,
        )
        .is_err());
    assert!(runtime
        .admit_owner_schedule_action_in_upstream_scope(
            "cwp://example.test/control",
            "wrong-activation",
            &person,
            &a,
            "@cwr--reload--graph",
            json!({}),
            None,
        )
        .is_err());
    let settled = runtime.execute_admission(&admission, |_| {}).unwrap();
    assert_eq!(
        settled.invocation.unwrap().state,
        ActionInvocationState::Completed
    );
    assert!(
        runtime
            .schedules_module_as(&a, &person)
            .unwrap()
            .get(&schedule.schedule_ref)
            .unwrap()
            .paused
    );
    let mut policy = runtime.lock().unwrap().admission.context_policy.clone().unwrap();
    policy.default_context_ref = a.clone();
    runtime.configure_conversation_contexts(Some(policy.clone())).unwrap();
    assert!(runtime
        .admit_owner_schedule_action_in_upstream_scope(
            "cwp://example.test/control",
            "owner-pause",
            &person,
            &a,
            "@cwr--pause--schedule",
            json!({"schedule_ref": schedule.schedule_ref}),
            None,
        )
        .is_err());
    assert!(runtime
        .admit_action_in_upstream_scope(
            "cwp://example.test/control",
            "owner-pause",
            &person,
            None,
            "@cwr--pause--schedule",
            json!({"schedule_ref": schedule.schedule_ref}),
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("schedule recovery requires its installed owner destination"));
    policy.default_context_ref = "cwp://example.test/workspace".into();
    policy.contexts.remove(&a);
    runtime.configure_conversation_contexts(Some(policy)).unwrap();
    assert!(runtime
        .admit_owner_schedule_action_in_upstream_scope(
            "cwp://example.test/control",
            "owner-pause",
            &person,
            &a,
            "@cwr--pause--schedule",
            json!({"schedule_ref": schedule.schedule_ref}),
            None,
        )
        .is_err());
    assert!(runtime
        .admit_action_in_upstream_scope(
            "cwp://example.test/control",
            "owner-pause",
            &person,
            None,
            "@cwr--pause--schedule",
            json!({"schedule_ref": schedule.schedule_ref}),
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("schedule recovery requires its installed owner destination"));
}

#[test]
fn schedules_spike_disabled_pack_controls_retain_authored_participant_identity() {
    let (_temporary, runtime, _, a, _) = installed_schedule_fixture();
    let person = participation_ref(&runtime, "person");
    let manager_ref = "cwp://org.example/resources/2b99f30848ddf656ae19df88c9f7a46d42efc6775bec2868836cb16eb68fff6f";
    let mut policy = {
        let mut state = runtime.lock().unwrap();
        let mut actor = state
            .graph
            .actors
            .iter()
            .find(|actor| actor.coworker_slug == "helper")
            .unwrap()
            .clone();
        actor.actor_ref = manager_ref.into();
        actor.coworker_slug = "installed_manager".into();
        actor.actor_name = Some("Installed manager".into());
        actor.default_actor_integration_ref = None;
        actor.native_tools.clear();
        state
            .catalog
            .retained_actors
            .push(local_actor_identity(&actor).unwrap());
        state.graph.actors.push(actor);
        state.admission.context_policy.clone().unwrap()
    };
    policy
        .contexts
        .get_mut(&a)
        .unwrap()
        .push(manager_ref.into());
    runtime
        .configure_conversation_contexts(Some(policy))
        .unwrap();
    let list = runtime
        .submit_message_in_context(
            "cwp://example.test/control",
            "inspect-before-disable",
            &person,
            None,
            "@cwr--list--schedules",
            vec![],
            None,
            Some(&a),
        )
        .unwrap();
    runtime.execute_admission(&list, |_| {}).unwrap();
    runtime
        .invite_coworker_as(list.conversation_ref(), &person, manager_ref)
        .unwrap();
    runtime
        .lock()
        .unwrap()
        .graph
        .actors
        .retain(|actor| actor.actor_ref != manager_ref);
    runtime
        .configure_installed_schedules_as(&person, &a, "disabled", vec![], 1000)
        .unwrap();
    let discovery = runtime
        .discovery_as(
            Some(list.conversation_ref()),
            &person,
            None,
            &IntrinsicCwrCommand::ListCoworkers,
        )
        .unwrap()
        .unwrap();
    assert!(
        serde_json::to_string(&discovery)
            .unwrap()
            .contains("@installed_manager")
    );
    let inspect = runtime
        .admit_action_as(
            "cwp://example.test/control",
            "inspect-after-disable",
            &person,
            Some(list.conversation_ref()),
            "@cwr--list--schedules",
            json!({}),
        )
        .unwrap();
    let result = runtime
        .execute_admission(&inspect, |_| {})
        .unwrap()
        .invocation
        .unwrap();
    assert_eq!(result.state, ActionInvocationState::Completed);
    assert_eq!(
        result.terminal_response().unwrap().result.as_ref().unwrap()[0]["availability"],
        "disabled"
    );
}

