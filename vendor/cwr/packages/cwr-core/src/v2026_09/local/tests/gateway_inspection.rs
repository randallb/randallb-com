fn gateway_inspection_workspace(root: &Path) -> PathBuf {
    let manifest = gateway_test_workspace(root);
    let directory = manifest.parent().unwrap().join("coworkers/gateway");
    fs::write(
        directory.join("INSPECTION_COWORKER.toml"),
        r#"type = "cwp://org.coworkerprotocol/2026-09/schemas/coworker"
name = "Inspection actor"
slug = "inspection_actor"
[[roles]]
path = "./ROLE.toml"
as = "behavior"
[[integrations]]
path = "./integration.toml"
as = "execution"
default = true
"#,
    )
    .unwrap();
    let mut source = fs::read_to_string(&manifest).unwrap();
    source.push_str(
        "\n[[imports]]\npath = './coworkers/gateway/INSPECTION_COWORKER.toml'\nas = 'inspection'\n",
    );
    fs::write(&manifest, source).unwrap();
    manifest
}

#[test]
fn gateway_invocation_inspection_requires_exact_retained_origin_route_and_input() {
    use crate::v2026_09::service::GatewayInvocationKind;

    let temporary = tempfile::tempdir().unwrap();
    let manifest = gateway_inspection_workspace(temporary.path());
    let person = load_default_local_acting_actor_ref(&manifest).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        manifest,
        temporary.path().join("runtime.sqlite"),
        LocalHostIdentity::new(&person, "cwp://example.test/integrations/gateway"),
    )
    .unwrap();
    let route = runtime
        .graph()
        .unwrap()
        .routes
        .iter()
        .find(|route| route.action_name == "handle--request")
        .unwrap()
        .clone();
    let activation = runtime
        .catalog()
        .unwrap()
        .actions
        .iter()
        .find(|action| {
            action.actor_ref == route.actor_ref && action.action_name == route.action_name
        })
        .unwrap()
        .activation
        .clone();
    let input = json!({"request":{"value":"test"},"ref":"r","requester_actor_ref":"a","parent_invocation_ref":"p"});
    let admission = runtime
        .admit_action_as(
            "cwp://example.test/integrations/gateway",
            "gateway-event",
            person,
            None,
            &activation,
            input,
        )
        .unwrap();
    let invocation_ref = admission.invocation_ref().unwrap().to_owned();
    let (integration_ref, event_ref, input) = {
        let mut state = runtime.lock().unwrap();
        let record = state
            .admission
            .invocation_record(&invocation_ref)
            .unwrap()
            .clone();
        state.graph.gateways[0].connection_ref = Some("cwp://example.test/connections/test".into());
        state.graph.gateways[0].ingress_route = Some(route.clone());
        let InvocationOrigin::Ingress {
            integration_ref,
            provider_event_ref,
        } = record.origin
        else {
            panic!("root must be ingress")
        };
        (integration_ref, provider_event_ref, record.input)
    };
    let matches = |integration: &str, event: &str, input: &Value, kind| {
        runtime
            .gateway_invocation_matches(
                "cwp://example.test/connections/test",
                integration,
                event,
                &invocation_ref,
                kind,
                input,
            )
            .unwrap()
    };
    assert!(matches(
        &integration_ref,
        &event_ref,
        &input,
        GatewayInvocationKind::Ingress
    ));
    assert!(!matches(
        "wrong",
        &event_ref,
        &input,
        GatewayInvocationKind::Ingress
    ));
    assert!(!matches(
        &integration_ref,
        "wrong",
        &input,
        GatewayInvocationKind::Ingress
    ));
    assert!(!matches(
        &integration_ref,
        &event_ref,
        &json!({"wrong": true}),
        GatewayInvocationKind::Ingress
    ));
    assert!(!matches(
        &integration_ref,
        &event_ref,
        &input,
        GatewayInvocationKind::Request
    ));
}

#[test]
fn gateway_inspection_recovers_terminal_result_after_runtime_reopen() {
    use crate::v2026_09::service::GatewayInvocationKind;

    let temporary = tempfile::tempdir().unwrap();
    let manifest = gateway_inspection_workspace(temporary.path());
    let gateway = manifest
        .parent()
        .unwrap()
        .join("coworkers/gateway/GATEWAY.toml");
    let source = fs::read_to_string(&gateway).unwrap();
    fs::write(
        &gateway,
        source.replace(
            "action_name = \"handle--request\"",
            "action_name = \"handle--request\"\nconnection_ref = 'gateway-replay'",
        ),
    )
    .unwrap();
    let person = load_default_local_acting_actor_ref(&manifest).unwrap();
    let identity = LocalHostIdentity::new(&person, "cwp://example.test/integrations/gateway");
    let state_path = temporary.path().join("runtime.sqlite");
    let runtime =
        LocalRuntime::open_with_state_path(&manifest, &state_path, identity.clone()).unwrap();
    let activation = runtime
        .catalog()
        .unwrap()
        .actions
        .iter()
        .find(|action| action.action_name == "handle--request")
        .unwrap()
        .activation
        .clone();
    let input = json!({
        "request": {"value": "test"},
        "ref": "r",
        "requester_actor_ref": "a",
        "parent_invocation_ref": "p"
    });
    let admission = runtime
        .admit_action_as(
            "cwp://example.test/integrations/gateway",
            "replayed-event",
            &person,
            None,
            &activation,
            input.clone(),
        )
        .unwrap();
    let invocation_ref = admission.invocation_ref().unwrap().to_owned();
    {
        let mut state = runtime.lock().unwrap();
        let route = state
            .graph
            .routes
            .iter()
            .find(|route| route.action_name == "handle--request")
            .unwrap()
            .clone();
        state.graph.gateways[0].ingress_route = Some(route);
    }
    assert_eq!(
        runtime
            .inspect_gateway_invocation(
                "gateway-replay",
                "cwp://example.test/integrations/gateway",
                "replayed-event",
                &invocation_ref,
                GatewayInvocationKind::Ingress,
                &input,
            )
            .unwrap(),
        (true, None)
    );
    {
        let mut state = runtime.lock().unwrap();
        state
            .admission
            .claim_invocation(&invocation_ref, 10)
            .unwrap();
        state
            .admission
            .finish_invocation(
                &invocation_ref,
                InvocationTerminal {
                    status: ActionResponseStatus::COMPLETED,
                    summary: "Done".into(),
                    result: Some(json!({"delivered": true})),
                    error: None,
                },
                20,
            )
            .unwrap();
    }
    drop(runtime);

    let reopened = LocalRuntime::open_with_state_path(&manifest, &state_path, identity).unwrap();
    {
        let mut state = reopened.lock().unwrap();
        let route = state
            .graph
            .routes
            .iter()
            .find(|route| route.action_name == "handle--request")
            .unwrap()
            .clone();
        state.graph.gateways[0].ingress_route = Some(route);
    }
    let (authorized, terminal) = reopened
        .inspect_gateway_invocation(
            "gateway-replay",
            "cwp://example.test/integrations/gateway",
            "replayed-event",
            &invocation_ref,
            GatewayInvocationKind::Ingress,
            &input,
        )
        .unwrap();
    assert!(authorized);
    let terminal = terminal.unwrap();
    assert_eq!(terminal.status, 200);
    assert_eq!(terminal.result, Some(json!({"delivered": true})));
    assert_eq!(
        reopened
            .inspect_gateway_invocation(
                "gateway-replay",
                "cwp://example.test/integrations/forged",
                "replayed-event",
                &invocation_ref,
                GatewayInvocationKind::Ingress,
                &input,
            )
            .unwrap(),
        (false, None)
    );
}
