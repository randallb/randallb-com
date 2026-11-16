use super::*;

fn copy_gateway_fixture(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_gateway_fixture(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn sms_gateway_state(temporary: &tempfile::TempDir) -> (SharedState, String, String) {
    let workspace = temporary.path().join("workspace");
    copy_gateway_fixture(fixture_workspace().parent().unwrap(), &workspace);
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    copy_gateway_fixture(
        &repository.join("coworkers/workloop_sms"),
        &workspace.join("coworkers/workloop_sms"),
    );
    fs::create_dir_all(workspace.join("integrations")).unwrap();
    fs::write(workspace.join("integrations/local_sh.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n").unwrap();
    let manifest = workspace.join("CWR.toml");
    let mut source = fs::read_to_string(&manifest).unwrap();
    source.push_str(
        "\n[[imports]]\npath = './gateways/workloop_sms/GATEWAY.toml'\nas = 'workloop_sms'\n",
    );
    fs::write(&manifest, source).unwrap();
    let gateway = workspace.join("gateways/workloop_sms");
    fs::create_dir_all(&gateway).unwrap();
    fs::write(
        gateway.join("GATEWAY.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/gateway'\nauthority = ['process.execute']\nrole = { ref = '~@/behavior' }\nintegration = { ref = '~@/local_sh' }\naction_name = 'handle--request'\ningress_action_name = 'receive--message'\ndelivery_action_name = 'deliver--message'\nconnection_ref = 'workloop-sms-pilot'\n[params]\nfail_on_nonzero = true\nstream_output = false\ntimeout_ms = 3600000\n[params.host_env_actions]\nreceive--message = 'WORKLOOP_SMS_BRIDGE_SOCKET'\ndeliver--message = 'WORKLOOP_SMS_BRIDGE_SOCKET'\nhandle--request = 'WORKLOOP_SMS_BRIDGE_SOCKET'\n[params.commands]\nreceive--message = '\"$CWR_CONTROL_EXECUTABLE\" sms-action receive--message'\ndeliver--message = '\"$CWR_CONTROL_EXECUTABLE\" sms-action deliver--message'\nhandle--request = '\"$CWR_CONTROL_EXECUTABLE\" sms-action handle--request'\n[[imports]]\npath = '../../coworkers/workloop_sms/ROLE.toml'\nas = 'behavior'\n[[imports]]\npath = '../../integrations/local_sh.toml'\nas = 'local_sh'\n",
    ).unwrap();
    let actor = cwr::v2026_09::local::load_default_local_acting_actor_ref(&manifest).unwrap();
    let runtime = LocalRuntime::open_with_state_path_and_runtime_ref(
        &manifest,
        temporary.path().join("sms-runtime.sqlite"),
        "cwp://example.test/runtime",
        LocalHostIdentity::new(&actor, "cwp://example.test/integrations/sms-host"),
    )
    .unwrap();
    let activation = "gateway--deliver--message".to_owned();
    let state = Arc::new(RuntimeServerShared {
        state: Mutex::new(RuntimeServerState {
            generation_restart: None,
            committed_retirement: None,
            armed_retirement: None,
            retirement_allowed: false,
            controls_finalizing: false,
            conversation_controls: BTreeMap::new(),
            runtime_ref: "cwp://example.test/runtime".into(),
            runtime,
            subscribers: Vec::new(),
            host_actors: BTreeMap::new(),
            communication_adapters: BTreeMap::new(),
            gateways: BTreeMap::new(),
            attachments: BTreeMap::new(),
            pending_default_conversation_refs: BTreeSet::new(),
            expected_communication_adapters: 0,
            attachment_state_path: temporary.path().join("attachments.json"),
            published_turn_statuses: BTreeMap::new(),
            published_stopped_invocations: std::collections::BTreeSet::new(),
            live_statuses: LiveStatusRegistry::default(),
            live_outputs: LiveOutputRegistry::default(),
        }),
        broadcast_lock: Mutex::new(()),
        graphql_changes: tokio::sync::watch::channel(()).0,
        scheduler: scheduler::ExecutionScheduler::default(),
        activity: crate::idle::Activity::default(),
    });
    (state, actor, activation)
}

fn next_command_event(reader: &mut BufReader<UnixStream>, request_id: &str) -> RuntimeServiceEvent {
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read service event");
        assert!(!line.is_empty(), "service connection closed");
        let event: RuntimeServiceEvent = serde_json::from_str(&line).unwrap();
        match &event {
            RuntimeServiceEvent::CommandSucceeded { request_id: id, .. }
            | RuntimeServiceEvent::CommandFailed { request_id: id, .. }
            | RuntimeServiceEvent::GatewayInvocationInspected { request_id: id, .. }
                if id == request_id =>
            {
                return event
            }
            _ => {}
        }
    }
}

#[test]
fn configured_sms_gateway_inspects_only_registered_writer_and_exact_action() {
    let temporary = tempfile::tempdir().unwrap();
    let (state, actor, activation) = sms_gateway_state(&temporary);
    // Drive the worker explicitly so replay does not rely on a fresh
    // broadcast from the scheduler after the service writer reconnects.
    state.scheduler.stop();
    let (client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let writer = client_writer(server).unwrap();
    let mut reader = BufReader::new(client);
    let (other_client, other_server) = UnixStream::pair().unwrap();
    other_client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let other = client_writer(other_server).unwrap();
    let mut other_reader = BufReader::new(other_client);
    let connection_ref = "workloop-sms-pilot";
    let integration_ref = "cwp://example.test/integrations/sms-host";
    let approval_fingerprint = lock_state(&state)
        .unwrap()
        .runtime
        .graph()
        .unwrap()
        .gateways
        .iter()
        .find(|gateway| gateway.connection_ref.as_deref() == Some(connection_ref))
        .unwrap()
        .approval_fingerprint()
        .unwrap();
    let register = |request_id: &str, ingress: Option<&str>, delivery: Option<&str>| {
        RuntimeServiceCommand::RegisterGateway {
            version: RUNTIME_SERVICE_VERSION,
            request_id: request_id.into(),
            connection_ref: connection_ref.into(),
            integration_ref: integration_ref.into(),
            approval_fingerprint: approval_fingerprint.clone(),
            ingress_action_name: ingress.map(str::to_owned),
            delivery_action_name: delivery.map(str::to_owned),
        }
    };
    assert!(
        execute_command(&state, &writer, register("missing-route", None, None))
            .unwrap_err()
            .to_string()
            .contains("requires ingress or delivery")
    );
    assert!(execute_command(
        &state,
        &writer,
        register("wrong-route", Some("receive--message"), Some("wrong")),
    )
    .unwrap_err()
    .to_string()
    .contains("differ from configured route"));
    execute_command(
        &state,
        &writer,
        register(
            "register",
            Some("receive--message"),
            Some("deliver--message"),
        ),
    )
    .unwrap();
    assert!(matches!(
        next_command_event(&mut reader, "register"),
        RuntimeServiceEvent::CommandSucceeded { .. }
    ));
    let input = serde_json::json!({"delivery":{
        "attachment_ref":"attachment-1", "delivery_ref":"delivery-1",
        "conversation_ref":"cwp://example.test/conversations/one", "message":{}
    }});
    assert_sms_submission_authority(
        &state,
        &writer,
        &actor,
        &activation,
        integration_ref,
        &input,
    );
    execute_command(
        &state,
        &writer,
        RuntimeServiceCommand::SubmitAction {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "sms-delivery-1".into(),
            integration_ref: integration_ref.into(),
            author_actor_ref: actor.clone(),
            conversation_ref: None,
            activation: activation.clone(),
            input: input.clone(),
        },
    )
    .unwrap();
    let RuntimeServiceEvent::CommandSucceeded {
        admission: Some(admission),
        ..
    } = next_command_event(&mut reader, "sms-delivery-1")
    else {
        panic!("action was not admitted")
    };
    let invocation_ref = admission.record.invocation_ref.unwrap();
    let inspect = |request_id: &str, input: serde_json::Value| {
        RuntimeServiceCommand::InspectGatewayInvocation {
            version: RUNTIME_SERVICE_VERSION,
            request_id: request_id.into(),
            connection_ref: connection_ref.into(),
            invocation_ref: invocation_ref.clone(),
            invocation_kind: GatewayInvocationKind::Delivery,
            input,
        }
    };
    execute_command(&state, &writer, inspect("valid", input.clone())).unwrap();
    assert!(matches!(
        next_command_event(&mut reader, "valid"),
        RuntimeServiceEvent::GatewayInvocationInspected {
            authorized: true,
            terminal: None,
            ..
        }
    ));
    execute_command(
        &state,
        &writer,
        inspect("forged-input", serde_json::json!({"delivery":{}})),
    )
    .unwrap();
    assert!(matches!(
        next_command_event(&mut reader, "forged-input"),
        RuntimeServiceEvent::GatewayInvocationInspected {
            authorized: false,
            terminal: None,
            ..
        }
    ));
    assert!(
        execute_command(&state, &other, inspect("wrong-writer", input.clone()))
            .unwrap_err()
            .to_string()
            .contains("another connection")
    );
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
        .unwrap();
    let retained = runtime
        .terminal_execution_for_invocation(&invocation_ref)
        .unwrap()
        .unwrap()
        .2;
    assert!(retained.status.is_terminal());
    assert_gateway_tracking_limit(&state, connection_ref, &invocation_ref);
    unsubscribe(&state, &writer);
    execute_command(
        &state,
        &other,
        register(
            "register-reconnected",
            Some("receive--message"),
            Some("deliver--message"),
        ),
    )
    .unwrap();
    assert!(matches!(
        next_command_event(&mut other_reader, "register-reconnected"),
        RuntimeServiceEvent::CommandSucceeded { .. }
    ));
    execute_command(
        &state,
        &other,
        RuntimeServiceCommand::SubmitAction {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "sms-delivery-1".into(),
            integration_ref: integration_ref.into(),
            author_actor_ref: actor,
            conversation_ref: None,
            activation,
            input: input.clone(),
        },
    )
    .unwrap();
    let RuntimeServiceEvent::CommandSucceeded {
        admission: Some(replayed),
        ..
    } = next_command_event(&mut other_reader, "sms-delivery-1")
    else {
        panic!("replay was not admitted")
    };
    assert_eq!(
        replayed.record.invocation_ref.as_deref(),
        Some(invocation_ref.as_str())
    );
    execute_command(&state, &other, inspect("reconnected", input)).unwrap();
    assert!(matches!(
        next_command_event(&mut other_reader, "reconnected"),
        RuntimeServiceEvent::GatewayInvocationInspected {
            authorized: true,
            terminal: Some(terminal),
            ..
        } if terminal.status == retained.status.as_u16() && terminal.result == retained.result
    ));
}

#[test]
fn configured_sms_gateway_rejects_changed_approved_definition_before_action() {
    let temporary = tempfile::tempdir().unwrap();
    let (state, actor, activation) = sms_gateway_state(&temporary);
    state.scheduler.stop();
    let (client, server) = UnixStream::pair().unwrap();
    let writer = client_writer(server).unwrap();
    let mut reader = BufReader::new(client);
    let connection_ref = "workloop-sms-pilot";
    let fingerprint = lock_state(&state)
        .unwrap()
        .runtime
        .graph()
        .unwrap()
        .gateways
        .iter()
        .find(|gateway| gateway.connection_ref.as_deref() == Some(connection_ref))
        .unwrap()
        .approval_fingerprint()
        .unwrap();
    let register =
        |request_id: &str, approval_fingerprint: String| RuntimeServiceCommand::RegisterGateway {
            version: RUNTIME_SERVICE_VERSION,
            request_id: request_id.into(),
            connection_ref: connection_ref.into(),
            integration_ref: "cwp://example.test/integrations/sms-host".into(),
            approval_fingerprint,
            ingress_action_name: Some("receive--message".into()),
            delivery_action_name: Some("deliver--message".into()),
        };
    assert!(
        execute_command(&state, &writer, register("wrong", "a".repeat(64)))
            .unwrap_err()
            .to_string()
            .contains("differs from private approval")
    );
    execute_command(&state, &writer, register("approved", fingerprint.clone())).unwrap();
    let _ = next_command_event(&mut reader, "approved");

    let gateway = temporary
        .path()
        .join("workspace/gateways/workloop_sms/GATEWAY.toml");
    let original = fs::read_to_string(&gateway).unwrap();
    assert!(original.contains("sms-action receive--message"));
    fs::write(
        &gateway,
        original.replace("sms-action receive--message", "sms-action handle--request"),
    )
    .unwrap();
    lock_state(&state).unwrap().runtime.reload_graph().unwrap();
    assert!(require_gateway_fingerprint(
        &lock_state(&state).unwrap().runtime,
        connection_ref,
        &fingerprint
    )
    .unwrap_err()
    .to_string()
    .contains("differs from private approval"));
    let submit = RuntimeServiceCommand::SubmitAction {
        version: RUNTIME_SERVICE_VERSION,
        request_id: "after-reload".into(),
        integration_ref: "cwp://example.test/integrations/sms-host".into(),
        author_actor_ref: actor,
        conversation_ref: None,
        activation,
        input: serde_json::json!({}),
    };
    assert!(execute_command(&state, &writer, submit)
        .unwrap_err()
        .to_string()
        .contains("differs from private approval"));
    let inspect = RuntimeServiceCommand::InspectGatewayInvocation {
        version: RUNTIME_SERVICE_VERSION,
        request_id: "inspect-after-reload".into(),
        connection_ref: connection_ref.into(),
        invocation_ref: "unknown".into(),
        invocation_kind: GatewayInvocationKind::Ingress,
        input: serde_json::json!({}),
    };
    assert!(execute_command(&state, &writer, inspect)
        .unwrap_err()
        .to_string()
        .contains("differs from private approval"));
}

#[test]
fn configured_sms_gateway_fingerprint_covers_all_routes_and_resolved_fields() {
    let temporary = tempfile::tempdir().unwrap();
    let (state, _, _) = sms_gateway_state(&temporary);
    state.scheduler.stop();
    let gateway = lock_state(&state)
        .unwrap()
        .runtime
        .graph()
        .unwrap()
        .gateways
        .into_iter()
        .find(|gateway| gateway.connection_ref.as_deref() == Some("workloop-sms-pilot"))
        .unwrap();
    let approved = gateway.approval_fingerprint().unwrap();
    let mut changed = gateway.clone();
    changed.route.actor_ref.push_str("-changed");
    assert_ne!(changed.approval_fingerprint().unwrap(), approved);
    let mut changed = gateway.clone();
    changed.ingress_route.as_mut().unwrap().input_schema = serde_json::json!({"type":"null"});
    assert_ne!(changed.approval_fingerprint().unwrap(), approved);
    let mut changed = gateway.clone();
    changed.delivery_route.as_mut().unwrap().integration_params =
        serde_json::json!({"changed":true});
    assert_ne!(changed.approval_fingerprint().unwrap(), approved);
    let mut changed = gateway.clone();
    changed
        .delivery_route
        .as_mut()
        .unwrap()
        .working_path
        .push("changed");
    assert_ne!(changed.approval_fingerprint().unwrap(), approved);
}

#[test]
fn gateway_registration_rejects_duplicates_other_writers_and_clears_on_disconnect() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let (_first_client, first_server) = UnixStream::pair().unwrap();
    let first = client_writer(first_server).unwrap();
    let (_other_client, other_server) = UnixStream::pair().unwrap();
    let other = client_writer(other_server).unwrap();
    let connection_ref = "cwp://example.test/connections/sms";
    lock_state(&state).unwrap().gateways.insert(
        connection_ref.into(),
        RegisteredGateway {
            integration_ref: "cwp://example.test/integrations/sms".into(),
            approval_fingerprint: "a".repeat(64),
            writer: Arc::clone(&first),
            issued: BTreeMap::new(),
        },
    );
    let duplicate = RuntimeServiceCommand::RegisterGateway {
        version: RUNTIME_SERVICE_VERSION,
        request_id: "register-again".into(),
        connection_ref: connection_ref.into(),
        integration_ref: "cwp://example.test/integrations/sms".into(),
        approval_fingerprint: "a".repeat(64),
        ingress_action_name: Some("receive--message".into()),
        delivery_action_name: Some("deliver--message".into()),
    };
    assert!(execute_command(&state, &other, duplicate)
        .unwrap_err()
        .to_string()
        .contains("already exists"));
    let inspect = RuntimeServiceCommand::InspectGatewayInvocation {
        version: RUNTIME_SERVICE_VERSION,
        request_id: "inspect".into(),
        connection_ref: connection_ref.into(),
        invocation_ref: "unknown".into(),
        invocation_kind: GatewayInvocationKind::Ingress,
        input: serde_json::json!({}),
    };
    assert!(execute_command(&state, &other, inspect.clone())
        .unwrap_err()
        .to_string()
        .contains("another connection"));
    assert!(execute_command(&state, &first, inspect)
        .unwrap_err()
        .to_string()
        .contains("approved gateway definition is unavailable"));
    unsubscribe(&state, &first);
    assert!(!lock_state(&state)
        .unwrap()
        .gateways
        .contains_key(connection_ref));
}

fn assert_sms_submission_authority(
    state: &SharedState,
    writer: &SharedWriter,
    actor: &str,
    activation: &str,
    integration_ref: &str,
    input: &serde_json::Value,
) {
    let unapproved_activation = "gateway--handle--request".to_owned();
    let wrong_entry = execute_command(
        state,
        writer,
        RuntimeServiceCommand::SubmitAction {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "wrong-entry".into(),
            integration_ref: integration_ref.into(),
            author_actor_ref: actor.to_owned(),
            conversation_ref: None,
            activation: unapproved_activation,
            input: serde_json::json!({}),
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        wrong_entry.contains("gateway activation differs from approved entry points"),
        "{wrong_entry}"
    );
    assert!(execute_command(
        state,
        writer,
        RuntimeServiceCommand::SubmitAction {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "forged-integration".into(),
            integration_ref: "cwp://example.test/integrations/forged".into(),
            author_actor_ref: actor.to_owned(),
            conversation_ref: None,
            activation: activation.to_owned(),
            input: input.clone(),
        },
    )
    .unwrap_err()
    .to_string()
    .contains("another integration"));
}

fn assert_gateway_tracking_limit(state: &SharedState, connection: &str, terminal: &str) {
    let mut locked = lock_state(state).unwrap();
    let runtime = locked.runtime.clone();
    let gateway = locked.gateways.get_mut(connection).unwrap();
    for index in 1..MAX_GATEWAY_INVOCATIONS {
        gateway
            .issued
            .insert(format!("in-flight-{index}"), format!("event-{index}"));
    }
    assert_eq!(gateway.issued.len(), MAX_GATEWAY_INVOCATIONS);
    reserve_gateway_invocation(&runtime, gateway, "next-event").unwrap();
    assert!(!gateway.issued.contains_key(terminal));
    assert_eq!(gateway.issued.len(), MAX_GATEWAY_INVOCATIONS - 1);
    gateway
        .issued
        .insert("last-in-flight".into(), "last-event".into());
    assert!(reserve_gateway_invocation(&runtime, gateway, "overflow").is_err());
    assert_eq!(gateway.issued.len(), MAX_GATEWAY_INVOCATIONS);
    // Repeated requests do not consume a new slot, even at capacity.
    reserve_gateway_invocation(&runtime, gateway, "last-event").unwrap();
    assert!(gateway.issued.contains_key("last-in-flight"));
}
