#[test]
fn native_gateway_admission_rejects_stale_contract_after_reload_and_registration() {
    let (temp, runtime, person) = native_gateway_fixture();
    let (sender, receiver) = std::sync::mpsc::channel();
    let writer: UpstreamWriter = Arc::new(move |event| {
        sender
            .send(event)
            .map_err(|error| invalid_error(error.to_string()))
    });
    let first_session = runtime
        .register_gateway_handlers(
            "desktop",
            vec![native_gateway_declaration(&runtime)],
            writer.clone(),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    // This is the snapshot handle_gateway_request takes before releasing its
    // lookup lock. Reload and registration happen before its admission lock.
    let old_route = runtime.graph().unwrap().gateways[0].route.clone();
    let caller = gateway_sms_parent(&runtime, &person, "stale-admission", Some("desktop"));
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "stale".into(),
        operation: "cwr.send_request".into(),
        input: json!({}),
    };
    let old_input = serde_json::to_value(GatewayRequest {
        r#type: GATEWAY_REQUEST_TYPE.into(),
        r#ref: "cwp://example.test/stale-admission".into(),
        requester_actor_ref: caller.target_actor_ref.clone(),
        parent_invocation_ref: caller.invocation_ref.clone(),
        resource_ref: old_route.actor_ref.clone(),
        request: json!({"value":"old"}),
    })
    .unwrap();
    let directory = temp.path().join("workspace/coworkers/gateway");
    let input_path = directory.join("input.json");
    let mut input: Value = serde_json::from_str(&fs::read_to_string(&input_path).unwrap()).unwrap();
    input["properties"]["request"]["properties"]["value"] = json!({"const":"current"});
    fs::write(input_path, input.to_string()).unwrap();
    let output_path = directory.join("output.json");
    let mut output: Value =
        serde_json::from_str(&fs::read_to_string(&output_path).unwrap()).unwrap();
    output["properties"]["echo"] = json!({"const":"current"});
    fs::write(output_path, output.to_string()).unwrap();
    runtime.reload_graph().unwrap();
    let current_session = runtime
        .register_gateway_handlers(
            "desktop",
            vec![native_gateway_declaration(&runtime)],
            writer,
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    assert_ne!(first_session, current_session);
    let current_route = runtime.graph().unwrap().gateways[0].route.clone();
    assert!(old_route.validate_input(&old_input).is_ok());
    assert!(current_route.validate_input(&old_input).is_err());
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    assert!(
        runtime
            .start_gateway_action(&call, &caller, &queue, &old_route, old_input.clone(),)
            .is_err(),
        "a cached contract must not admit work through its replacement"
    );
    assert!(queue.borrow().is_empty());
    assert!(receiver.try_recv().is_err());
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_records()
            .count(),
        1
    );

    let mut current_input = old_input;
    current_input["request"]["value"] = json!("current");
    let fresh_call = AdapterCall {
        call_id: "fresh".into(),
        ..call
    };
    let AdapterHostCallOutcome::Returned(started) = runtime
        .start_gateway_action(&fresh_call, &caller, &queue, &current_route, current_input)
        .unwrap();
    let child: StartActionResult = serde_json::from_value(started).unwrap();
    {
        let state = runtime.lock().unwrap();
        let retained = state
            .admission
            .execution_conversation(&child.invocation_ref)
            .unwrap();
        let snapshot = retained
            .activity
            .iter()
            .find_map(|activity| match &activity.event {
                ConversationActivityEvent::TurnStarted {
                    invocation: Some(snapshot),
                    ..
                } => Some(snapshot),
                _ => None,
            })
            .unwrap();
        assert_eq!(snapshot["output_schema"], current_route.output_schema);
    }
    let executing = runtime.clone();
    let reference = child.invocation_ref.clone();
    let worker = thread::spawn(move || {
        executing.execute_requested_ref(
            &reference,
            &AdapterModelOutputMailbox::discard(),
            &mut |_| {},
        )
    });
    let UpstreamEvent::WorkRequested { work } =
        receiver.recv_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("expected only fresh work");
    };
    assert_eq!(work.route.provider_session_ref, current_session);
    assert_eq!(work.input["request"]["value"], "current");
    assert_eq!(
        work.route.coworker.actions[0].input_schema,
        current_route.input_schema
    );
    assert_eq!(
        work.route.coworker.actions[0].output_schema,
        current_route.output_schema
    );
    let mut terminal = native_success(&child.invocation_ref);
    assert!(runtime
        .complete_upstream_work("desktop", &child.invocation_ref, terminal.clone())
        .is_err());
    terminal.result.as_mut().unwrap()["echo"] = json!("current");
    runtime
        .complete_upstream_work("desktop", &child.invocation_ref, terminal)
        .unwrap();
    worker.join().unwrap().unwrap();
}

#[test]
fn gateway_admission_rejects_cached_route_across_execution_mode_changes() {
    for transition in ["shell_to_native", "native_to_shell", "shell_removed"] {
        let (temp, runtime, person) = native_gateway_fixture();
        let gateway_path = temp.path().join("workspace/coworkers/gateway/GATEWAY.toml");
        if transition != "native_to_shell" {
            let definition = fs::read_to_string(&gateway_path).unwrap();
            fs::write(
                &gateway_path,
                definition.replace("host_gateway = true", "host_gateway = false"),
            )
            .unwrap();
            runtime.reload_graph().unwrap();
        } else {
            runtime
                .register_gateway_handlers(
                    "desktop",
                    vec![native_gateway_declaration(&runtime)],
                    Arc::new(|_| Ok(())),
                    std::sync::mpsc::sync_channel(1).0,
                )
                .unwrap();
        }
        let cached = runtime.graph().unwrap().gateways[0].route.clone();
        let caller = gateway_sms_parent(&runtime, &person, transition, Some("desktop"));
        let call = AdapterCall {
            invocation_ref: caller.invocation_ref.clone(),
            call_id: transition.into(),
            operation: "cwr.send_request".into(),
            input: json!({}),
        };
        let envelope = serde_json::to_value(GatewayRequest {
            r#type: GATEWAY_REQUEST_TYPE.into(),
            r#ref: format!("cwp://example.test/{transition}"),
            requester_actor_ref: caller.target_actor_ref.clone(),
            parent_invocation_ref: caller.invocation_ref.clone(),
            resource_ref: cached.actor_ref.clone(),
            request: json!({"value":"synthetic"}),
        })
        .unwrap();
        if transition == "shell_removed" {
            let manifest = temp.path().join("workspace/CWR.toml");
            let definition = fs::read_to_string(&manifest).unwrap();
            let import =
                "\n[[imports]]\npath = \"./coworkers/gateway/GATEWAY.toml\"\nas = \"gateway\"\n";
            assert!(definition.contains(import));
            fs::write(manifest, definition.replace(import, "")).unwrap();
        } else {
            let definition = fs::read_to_string(&gateway_path).unwrap();
            fs::write(
                &gateway_path,
                if transition == "shell_to_native" {
                    definition.replace("host_gateway = false", "host_gateway = true")
                } else {
                    definition.replace("host_gateway = true", "")
                },
            )
            .unwrap();
        }
        runtime.reload_graph().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        if transition == "shell_to_native" {
            runtime
                .register_gateway_handlers(
                    "desktop",
                    vec![native_gateway_declaration(&runtime)],
                    Arc::new(move |event| {
                        sender
                            .send(event)
                            .map_err(|error| invalid_error(error.to_string()))
                    }),
                    std::sync::mpsc::sync_channel(1).0,
                )
                .unwrap();
        }
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        assert!(
            runtime
                .start_gateway_action(&call, &caller, &queue, &cached, envelope)
                .is_err(),
            "cached route admitted after {transition}"
        );
        assert!(queue.borrow().is_empty());
        assert!(receiver.try_recv().is_err());
        assert_eq!(
            runtime
                .lock()
                .unwrap()
                .admission
                .invocation_records()
                .count(),
            1
        );
    }
}

#[test]
fn availability_gateway_checks_caller_before_executor_availability() {
    let (_temp, runtime, person) = native_gateway_fixture();
    let caller = gateway_sms_parent(&runtime, &person, "context-gateway", Some("desktop"));
    let route = runtime.graph().unwrap().gateways[0].route.clone();
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "context-gateway".into(),
        operation: "cwr.send_request".into(),
        input: json!({}),
    };
    let envelope = serde_json::to_value(GatewayRequest {
        r#type: GATEWAY_REQUEST_TYPE.into(),
        r#ref: "cwp://example.test/context-gateway".into(),
        requester_actor_ref: caller.target_actor_ref.clone(),
        parent_invocation_ref: caller.invocation_ref.clone(),
        resource_ref: route.actor_ref.clone(),
        request: json!({"value":"synthetic"}),
    })
    .unwrap();
    let policy = |members| crate::v2026_09::upstream::ConversationContextPolicy {
        action_permissions: Vec::new(),
        suspended: false,
        default_context_ref: "workspace".into(),
        contexts: BTreeMap::from([("workspace".into(), members)]),
    };
    runtime
        .configure_conversation_contexts(Some(policy(vec![])))
        .unwrap();
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    // The configured native endpoint has no registered executor. Caller policy
    // must reject the request before revealing that execution availability.
    assert!(runtime
        .start_gateway_action(&call, &caller, &queue, &route, envelope.clone(),)
        .unwrap_err()
        .is::<ConversationContextError>());
    assert!(queue.borrow().is_empty());
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_records()
            .count(),
        1
    );

    // A gateway is a transport endpoint, not a context-member coworker. Only
    // restore the caller; the endpoint itself remains outside the context.
    runtime
        .configure_conversation_contexts(Some(policy(vec![caller.target_actor_ref.clone()])))
        .unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    runtime
        .register_gateway_handlers(
            "desktop",
            vec![native_gateway_declaration(&runtime)],
            Arc::new(move |event| {
                sender
                    .send(event)
                    .map_err(|error| invalid_error(error.to_string()))
            }),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    let AdapterHostCallOutcome::Returned(started) = runtime
        .start_gateway_action(&call, &caller, &queue, &route, envelope)
        .unwrap();
    let child: StartActionResult = serde_json::from_value(started).unwrap();
    let executing = runtime.clone();
    let reference = child.invocation_ref.clone();
    let worker = thread::spawn(move || {
        executing.execute_requested_ref(
            &reference,
            &AdapterModelOutputMailbox::discard(),
            &mut |_| {},
        )
    });
    let UpstreamEvent::WorkRequested { work } =
        receiver.recv_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("expected authorized gateway work");
    };
    assert_eq!(work.invocation_ref, child.invocation_ref);
    runtime
        .complete_upstream_work(
            "desktop",
            &child.invocation_ref,
            native_success(&child.invocation_ref),
        )
        .unwrap();
    worker.join().unwrap().unwrap();
}
