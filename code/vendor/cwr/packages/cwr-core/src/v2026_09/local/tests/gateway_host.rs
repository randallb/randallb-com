fn native_gateway_fixture() -> (tempfile::TempDir, LocalRuntime, String) {
    let temp = tempfile::tempdir().unwrap();
    let manifest = gateway_test_workspace(temp.path());
    let gateway = manifest
        .parent()
        .unwrap()
        .join("coworkers/gateway/GATEWAY.toml");
    let mut text = fs::read_to_string(&gateway).unwrap();
    text.push_str("\n[params]\nhost_gateway = true\n");
    fs::write(gateway, text).unwrap();
    let person = load_default_local_acting_actor_ref(&manifest).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        &manifest,
        temp.path().join("runtime.sqlite"),
        LocalHostIdentity::new(person.clone(), "cwp://example.test/host"),
    )
    .unwrap();
    (temp, runtime, person)
}

fn native_gateway_declaration(
    runtime: &LocalRuntime,
) -> super::super::upstream::UpstreamGatewayHandler {
    let graph = runtime.graph().unwrap();
    let gateway = &graph.gateways[0];
    super::super::upstream::UpstreamGatewayHandler {
        resource_ref: gateway.resource_ref.clone(),
        action_name: gateway.route.action_name.clone(),
        input_schema: gateway.route.input_schema.clone(),
        output_schema: gateway.route.output_schema.clone(),
    }
}

#[test]
fn smolvm_opt_in_cannot_register_a_native_host_gateway() {
    let (temp, runtime, person) = native_gateway_fixture();
    let gateway = temp.path().join("workspace/coworkers/gateway/GATEWAY.toml");
    let mut definition = fs::read_to_string(&gateway).unwrap();
    definition.push_str("execution_environment = \"smolvm\"\n");
    fs::write(&gateway, &definition).unwrap();
    runtime.reload_graph().unwrap();

    assert!(runtime
        .register_gateway_handlers(
            "desktop",
            vec![native_gateway_declaration(&runtime)],
            Arc::new(|_| Ok(())),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap_err()
        .to_string()
        .contains("SmolVM opt-in cannot use native host gateway execution"));
    assert!(runtime.lock().unwrap().gateway_handlers.is_empty());

    // An opted-in gateway without native host execution can still use the
    // ordinary local Shell route selected later by CWR.
    fs::write(
        &gateway,
        definition.replace("host_gateway = true", "host_gateway = false"),
    )
    .unwrap();
    runtime.reload_graph().unwrap();
    let caller = gateway_sms_parent(&runtime, &person, "smolvm-shell", Some("desktop"));
    let configured = runtime.graph().unwrap().gateways[0].route.clone();
    assert!(
        native_gateway_route(&runtime.lock().unwrap(), &caller, &configured)
            .unwrap()
            .is_none()
    );
}

#[test]
fn native_gateway_is_hidden_contract_bound_and_connection_owned() {
    let (_temp, runtime, person) = native_gateway_fixture();
    let declaration = native_gateway_declaration(&runtime);
    let mut wrong = declaration.clone();
    wrong.input_schema = json!({});
    let writer: UpstreamWriter = Arc::new(|_| Ok(()));
    let stop = || std::sync::mpsc::sync_channel(1).0;
    assert!(runtime
        .register_gateway_handlers("desktop", vec![wrong], writer.clone(), stop())
        .is_err());
    let session = runtime
        .register_gateway_handlers("desktop", vec![declaration.clone()], writer.clone(), stop())
        .unwrap();
    assert_eq!(
        runtime
            .register_gateway_handlers("desktop", vec![declaration.clone()], writer.clone(), stop())
            .unwrap(),
        session
    );
    assert!(runtime
        .upstream_catalog_as(&person, Some("desktop"))
        .unwrap()
        .actor_by_handle("@workloop_ops")
        .is_none());
    assert!(runtime
        .lock()
        .unwrap()
        .upstream
        .visible(Some("desktop"))
        .unwrap()
        .is_empty());
    assert!(runtime
        .enable_upstream_coworker(
            "desktop",
            &session,
            &declaration.resource_ref,
            &person,
            true
        )
        .is_err());
    let caller = gateway_sms_parent(&runtime, &person, "owned", Some("desktop"));
    let configured = runtime.graph().unwrap().gateways[0].route.clone();
    assert!(
        native_gateway_route(&runtime.lock().unwrap(), &caller, &configured)
            .unwrap()
            .is_some()
    );
    for (event, owner) in [("no-owner", None), ("other-owner", Some("other"))] {
        let caller = gateway_sms_parent(&runtime, &person, event, owner);
        assert!(native_gateway_route(&runtime.lock().unwrap(), &caller, &configured).is_err());
        let AdapterHostCallOutcome::Returned(result) = runtime.handle_host_call(
            AdapterCall { invocation_ref: caller.invocation_ref.clone(), call_id: event.into(),
                operation: "cwr.send_request".into(),
                input: json!({"type":GATEWAY_REQUEST_TYPE,"resource_ref":declaration.resource_ref,"request":{"value":"denied"}}),
            }, &caller, &Rc::new(RefCell::new(VecDeque::new())),
            &AdapterModelOutputMailbox::discard(), &mut |_| {},
        ).unwrap();
        assert_eq!(result["status"], 403, "{result}");
    }
    runtime.lock().unwrap().graph.gateways[0]
        .route
        .action_description
        .push_str(" changed");
    assert!(native_gateway_route(&runtime.lock().unwrap(), &caller, &configured).is_err());
    runtime.disconnect_upstream_provider("desktop").unwrap();
    assert!(runtime.lock().unwrap().gateway_handlers.is_empty());
    assert!(native_gateway_route(&runtime.lock().unwrap(), &caller, &configured).is_err());
    runtime.lock().unwrap().graph.gateways.clear();
    assert!(native_gateway_route(&runtime.lock().unwrap(), &caller, &configured).is_err());
}

#[test]
fn native_gateway_executes_without_ops_and_retains_original_consuming_turn() {
    let (_temp, runtime, person) = native_gateway_fixture();
    let declaration = native_gateway_declaration(&runtime);
    let resource = declaration.resource_ref.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    runtime
        .register_gateway_handlers(
            "desktop",
            vec![declaration],
            Arc::new(move |event| {
                sender
                    .send(event)
                    .map_err(|error| invalid_error(error.to_string()))
            }),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    let caller = gateway_sms_parent(&runtime, &person, "native-sms", Some("desktop"));
    let expected_caller = caller.clone();
    let host_runtime = runtime.clone();
    let worker = thread::spawn(move || {
        let UpstreamEvent::WorkRequested { work } =
            receiver.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("expected native gateway work");
        };
        assert_eq!(work.action_name, "handle--request");
        assert_eq!(work.route.coworker.actor_ref, resource);
        assert_eq!(
            work.route.gateway_resource_ref.as_deref(),
            Some(resource.as_str())
        );
        assert_eq!(work.input["resource_ref"], resource);
        assert_eq!(
            work.input["requester_actor_ref"],
            expected_caller.target_actor_ref
        );
        assert_eq!(
            work.input["parent_invocation_ref"],
            expected_caller.invocation_ref
        );
        assert_eq!(work.root_invocation_ref, expected_caller.invocation_ref);
        assert_eq!(work.input["request"]["value"], "synthetic");
        host_runtime
            .protect_upstream_result("desktop", &work.invocation_ref)
            .unwrap();
        assert!(host_runtime
            .protect_upstream_result("other", &work.invocation_ref)
            .is_err());
        host_runtime
            .complete_upstream_work(
                "desktop",
                &work.invocation_ref,
                AdapterTerminal {
                    invocation_ref: work.invocation_ref.clone(),
                    sequence: 1,
                    status: 200,
                    summary: "Synthetic read".into(),
                    result: Some(
                        json!({"echo":"synthetic","pid":0,"ref":"synthetic","caller":"synthetic"}),
                    ),
                    error: None,
                    continuation: None,
                },
            )
            .unwrap();
        work.invocation_ref
    });
    let resource = runtime.graph().unwrap().gateways[0].resource_ref.clone();
    let outcome = runtime.handle_host_call(AdapterCall {
        invocation_ref: caller.invocation_ref.clone(), call_id: "native".into(),
        operation: "cwr.send_request".into(),
        input: json!({"type":GATEWAY_REQUEST_TYPE,"resource_ref":resource,"request":{"value":"synthetic"}}),
    }, &caller, &Rc::new(RefCell::new(VecDeque::new())), &AdapterModelOutputMailbox::discard(), &mut |_| {}).unwrap();
    let AdapterHostCallOutcome::Returned(result) = outcome;
    assert_eq!(result["status"], 200, "{result}");
    assert_eq!(result["result"]["echo"], "synthetic");
    let child = worker.join().unwrap();
    let state = runtime.lock().unwrap();
    assert!(state
        .admission
        .invocation_record(&child)
        .unwrap()
        .state
        .is_terminal());
    assert!(state.protected_upstream_results[&caller.invocation_ref].contains_key(&child));
    assert!(!state
        .admission
        .conversation(caller.conversation_ref.as_deref().unwrap())
        .unwrap()
        .participant_actor_refs
        .contains(&resource));
}

#[test]
fn ordinary_upstream_gateway_discriminator_does_not_acquire_native_classification() {
    let (temp, runtime, person) = native_gateway_fixture();
    let mut host = gateway_mail_host();
    host.actions[0].input_schema = json!({"type":"object", "required":["type"],
        "properties":{"type":{"const":GATEWAY_REQUEST_TYPE}, "resource_ref":{"type":"string"}},
        "additionalProperties":false});
    let (sender, receiver) = std::sync::mpsc::channel();
    let session = runtime
        .register_upstream_provider(
            "desktop",
            vec![host.clone()],
            Arc::new(move |event| {
                sender
                    .send(event)
                    .map_err(|error| invalid_error(error.to_string()))
            }),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    runtime
        .enable_upstream_coworker("desktop", &session, &host.actor_ref, &person, true)
        .unwrap();
    let caller = gateway_sms_parent(&runtime, &person, "ordinary-discriminator", Some("desktop"));
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    let AdapterHostCallOutcome::Returned(start) = runtime
        .handle_start_action(
            &AdapterCall {
                invocation_ref: caller.invocation_ref.clone(),
                call_id: "ordinary".into(),
                operation: START_ACTION_HOST_OPERATION.into(),
                input: json!({"activation":"@workloop_ops--read--imap", "input":{
            "type":GATEWAY_REQUEST_TYPE,"resource_ref":"cwp://example.test/not-a-native-gateway"}}),
            },
            &caller,
            &queue,
        )
        .unwrap();
    let child: StartActionResult = serde_json::from_value(start).unwrap();
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
        panic!("expected ordinary upstream work")
    };
    assert_eq!(work.input["type"], GATEWAY_REQUEST_TYPE);
    assert_eq!(work.route.gateway_resource_ref, None);
    runtime
        .protect_upstream_result("desktop", &child.invocation_ref)
        .unwrap();
    change_native_gateway(&temp, &runtime, false);
    assert!(
        receiver.try_recv().is_err(),
        "ordinary upstream work must not be aborted by a gateway change"
    );
    runtime
        .complete_upstream_work(
            "desktop",
            &child.invocation_ref,
            AdapterTerminal {
                invocation_ref: child.invocation_ref.clone(),
                sequence: 1,
                status: 200,
                summary: "Ordinary action completed".into(),
                result: Some(json!({"ok":true})),
                error: None,
                continuation: None,
            },
        )
        .unwrap();
    worker.join().unwrap().unwrap();
    let state = runtime.lock().unwrap();
    let record = state
        .admission
        .invocation_record(&child.invocation_ref)
        .unwrap();
    assert!(record.terminal_response().unwrap().status.is_completed());
    require_native_gateway_publication_current(&state, &caller).unwrap();
}

fn cloud_host_result_fixture(
    status: u16,
) -> (
    tempfile::TempDir,
    LocalRuntime,
    String,
    ActionInvocationRecord,
    String,
) {
    let (temp, runtime, person) = native_gateway_fixture();
    let declaration = native_gateway_declaration(&runtime);
    let resource = declaration.resource_ref.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    runtime
        .register_gateway_handlers(
            "desktop",
            vec![declaration],
            Arc::new(move |event| {
                sender
                    .send(event)
                    .map_err(|error| invalid_error(error.to_string()))
            }),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    let caller = gateway_sms_parent(&runtime, &person, "cloud-result", Some("desktop"));
    let host = runtime.clone();
    let worker = thread::spawn(move || {
        let UpstreamEvent::WorkRequested { work } =
            receiver.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("host work")
        };
        host.complete_upstream_work(
            "desktop",
            &work.invocation_ref,
            AdapterTerminal {
                invocation_ref: work.invocation_ref.clone(),
                sequence: 1,
                status,
                summary: "Fictional result".into(),
                result: (status == 200).then(
                    || json!({"echo":"synthetic","pid":0,"ref":"synthetic","caller":"synthetic"}),
                ),
                error: (status != 200).then(|| crate::adapter::AdapterError {
                    code: "fixture_failed".into(),
                    message: Some("Fictional failure".into()),
                    details: None,
                }),
                continuation: None,
            },
        )
        .unwrap();
        work.invocation_ref
    });
    let _outcome = runtime.handle_host_call(AdapterCall {invocation_ref:caller.invocation_ref.clone(),call_id:"cloud-result".into(),operation:"cwr.send_request".into(),input:json!({"type":GATEWAY_REQUEST_TYPE,"resource_ref":resource,"request":{"value":"synthetic"}})},&caller,&Rc::new(RefCell::new(VecDeque::new())),&AdapterModelOutputMailbox::discard(),&mut |_|{}).unwrap();
    let child = worker.join().unwrap();
    (temp, runtime, person, caller, child)
}

#[test]
fn cloud_host_result_authority_accepts_completed_child_but_never_execution() {
    let (_temp, runtime, person, root, child) = cloud_host_result_fixture(200);
    let conversation = root.conversation_ref.as_deref().unwrap();
    assert!(runtime
        .authorize_cloud_host_work(&child, &person, conversation)
        .is_err());
    let proof = runtime
        .authorize_cloud_host_result(&child, &person, conversation)
        .unwrap();
    assert_eq!(proof["child_status"], 200);
    assert_eq!(proof["root_invocation_ref"], root.invocation_ref);
    assert_eq!(
        proof["origin_integration_ref"],
        "cwp://example.test/sms/account-assignment"
    );
    assert!(runtime
        .authorize_cloud_host_result(&child, &root.target_actor_ref, conversation)
        .is_err());
    let foreign = gateway_sms_parent(&runtime, &person, "foreign-result-root", Some("desktop"));
    assert!(runtime
        .authorize_cloud_host_result(
            &child,
            &person,
            foreign.conversation_ref.as_deref().unwrap()
        )
        .is_err());
    runtime
        .lock()
        .unwrap()
        .admission
        .complete(
            &root.invocation_ref,
            json!({"text":"Fictional final reply"}),
        )
        .unwrap();
    assert!(runtime
        .authorize_cloud_host_result(&child, &person, conversation)
        .is_ok());
    runtime
        .configure_conversation_contexts(Some(
            crate::v2026_09::upstream::ConversationContextPolicy {
                action_permissions: Vec::new(),
                suspended: true,
                default_context_ref: "closed".into(),
                contexts: BTreeMap::from([("closed".into(), Vec::new())]),
            },
        ))
        .unwrap();
    assert!(runtime
        .authorize_cloud_host_result(&child, &person, conversation)
        .is_err());
}

#[test]
fn cloud_host_result_authority_rejects_failed_child() {
    let (_temp, runtime, person, root, child) = cloud_host_result_fixture(503);
    assert!(runtime
        .authorize_cloud_host_result(&child, &person, root.conversation_ref.as_deref().unwrap())
        .is_err());
}
