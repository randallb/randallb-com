#[test]
fn configured_gateway_rejects_unknown_role_and_unsupported_access() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = gateway_test_workspace(temp.path());
    let graph = crate::v2026_09::graph::resolve_graph(&manifest).unwrap();
    assert_eq!(graph.gateways.len(), 1);
    assert_eq!(
        crate::v2026_09::graph::check(&manifest).unwrap()["gateways"][0]["resource_ref"],
        graph.gateways[0].resource_ref
    );
    let gateway = manifest
        .parent()
        .unwrap()
        .join("coworkers/gateway/GATEWAY.toml");
    let original = fs::read_to_string(&gateway).unwrap();
    fs::write(&gateway, original.replace("~@/behavior", "~@/missing")).unwrap();
    assert!(crate::v2026_09::graph::resolve_graph(&manifest).is_err());
    fs::write(
        &gateway,
        original.replace(
            "action_name = \"handle--request\"",
            "action_name = \"handle--request\"\n[access]\ncredential='unsupported'\n",
        ),
    )
    .unwrap();
    assert!(crate::v2026_09::graph::resolve_graph(&manifest).is_err());
}

#[test]
fn configured_gateway_validates_optional_host_entry_points() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = gateway_test_workspace(temp.path());
    let role = manifest
        .parent()
        .unwrap()
        .join("coworkers/gateway/ROLE.toml");
    let original_role = fs::read_to_string(&role).unwrap();
    fs::write(&role, format!("{original_role}\n[[actions]]\nverb='receive'\nobject='message'\ntitle='Receive message'\ndescription='Receive message'\n[actions.input_schema]\npath='./input.json'\n[actions.output_schema]\npath='./output.json'\n[[actions]]\nverb='deliver'\nobject='message'\ntitle='Deliver message'\ndescription='Deliver message'\n[actions.input_schema]\npath='./input.json'\n[actions.output_schema]\npath='./output.json'\n")).unwrap();
    let gateway = manifest
        .parent()
        .unwrap()
        .join("coworkers/gateway/GATEWAY.toml");
    let original = fs::read_to_string(&gateway).unwrap();
    let configured = original.replace("action_name = \"handle--request\"", "action_name = \"handle--request\"\ningress_action_name='receive--message'\ndelivery_action_name='deliver--message'\nconnection_ref='workloop-sms-pilot'");
    fs::write(&gateway, &configured).unwrap();
    let graph = crate::v2026_09::graph::resolve_graph(&manifest).unwrap();
    assert_eq!(
        graph.gateways[0]
            .ingress_route
            .as_ref()
            .unwrap()
            .action_name,
        "receive--message"
    );
    assert_eq!(
        graph.gateways[0]
            .delivery_route
            .as_ref()
            .unwrap()
            .action_name,
        "deliver--message"
    );
    assert_eq!(
        graph.gateways[0].connection_ref.as_deref(),
        Some("workloop-sms-pilot")
    );
    let checked = crate::v2026_09::graph::check(&manifest).unwrap();
    assert_eq!(
        checked["gateways"][0]["ingress_action_name"],
        "receive--message"
    );
    for invalid_fields in [
        "ingress_action_name='missing--action'\nconnection_ref='workloop-sms-pilot'\n",
        "ingress_action_name='receive--message'\n",
        "ingress_action_name='receive--message'\ndelivery_action_name='receive--message'\nconnection_ref='workloop-sms-pilot'\n",
        "connection_ref=''\n",
    ] {
        fs::write(&gateway, original.replace("action_name = \"handle--request\"", &format!("action_name = \"handle--request\"\n{invalid_fields}"))).unwrap();
        assert!(crate::v2026_09::graph::resolve_graph(&manifest).is_err());
    }
}

#[test]
fn configured_gateway_resolves_imported_role_in_nested_graph() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = gateway_test_workspace(temp.path());
    let root = fs::read_to_string(&manifest).unwrap();
    let root = root.replace(
        "[[imports]]\npath = \"./coworkers/gateway/GATEWAY.toml\"\nas = \"gateway\"\n",
        "[[imports]]\npath = './nested/CWR.toml'\nas = 'nested'\n",
    );
    fs::write(&manifest, root).unwrap();
    let nested = manifest.parent().unwrap().join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        nested.join("CWR.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath = '../coworkers/gateway/GATEWAY.toml'\nas = 'echo'\n",
    )
    .unwrap();
    let graph = crate::v2026_09::graph::resolve_graph(&manifest).unwrap();
    assert_eq!(graph.gateways.len(), 1);
    let resource_ref = &graph.gateways[0].resource_ref;
    assert!(resource_ref.starts_with("cwp://"), "{resource_ref}");
    assert!(!resource_ref.starts_with("~@/"), "{resource_ref}");
    assert_eq!(graph.gateways[0].route.action_name, "handle--request");
    let role = graph
        .resources
        .iter()
        .find(|resource| resource.source.ends_with("coworkers/gateway/ROLE.toml"))
        .unwrap();
    assert_eq!(graph.gateways[0].route.role_ref, role.reference);
}

#[test]
fn gateway_tool_describes_only_the_callers_permitted_sources() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = gateway_test_workspace(temp.path());
    let graph = crate::v2026_09::graph::resolve_graph(workspace).unwrap();
    let gateway = &graph.gateways[0];
    let allowed = BTreeSet::from([gateway.resource_ref.clone()]);
    let operation = permitted_gateway_operation(&graph, Some(&allowed));
    let refs = &operation.input_schema["properties"]["resource_ref"]["oneOf"];
    assert_eq!(refs.as_array().unwrap().len(), 1);
    assert_eq!(refs[0]["const"], gateway.resource_ref);
    assert_eq!(refs[0]["title"], gateway.route.action_title);
    let revoked = permitted_gateway_operation(&graph, Some(&BTreeSet::new()));
    assert_eq!(revoked.input_schema["properties"]["resource_ref"], false);
}

fn assert_gateway_request_allowlist(
    runtime: &LocalRuntime,
    caller: &ActionInvocationRecord,
    permitted_ref: &str,
    denied_ref: &str,
    input: &Value,
    invoke: &impl Fn(&str, Value) -> Value,
) {
    let (original, denied_aliases) = {
        let mut state = runtime.lock().unwrap();
        let denied_aliases = state.graph.gateways.iter()
            .find(|gateway| gateway.resource_ref == denied_ref).unwrap().request_refs.clone();
        let binding = state.graph.actor_integrations.iter_mut()
            .find(|binding| binding.actor_ref == caller.target_actor_ref
                && Some(binding.actor_integration_ref.as_str()) == caller.actor_integration_ref.as_deref()).unwrap();
        let original = binding.actor_integration_params.clone();
        binding.actor_integration_params["cwr_action_actor_refs"] = json!([permitted_ref]);
        (original, denied_aliases)
    };
    assert_eq!(invoke("allowlisted", input.clone())["status"], 200);
    let before = runtime.lock().unwrap().admission.invocation_records().count();
    for (index, reference) in denied_aliases.iter().enumerate() {
        let mut denied = input.clone();
        denied["resource_ref"] = json!(reference);
        let result = invoke(&format!("denied-{index}"), denied);
        assert_eq!(result["status"], 403, "{result}");
        assert_eq!(result["error"]["code"], "gateway_unavailable");
    }
    assert_eq!(runtime.lock().unwrap().admission.invocation_records().count(), before,
        "denied requests must not admit a child");
    {
        let mut state = runtime.lock().unwrap();
        let binding = state.graph.actor_integrations.iter_mut()
            .find(|binding| binding.actor_ref == caller.target_actor_ref
                && Some(binding.actor_integration_ref.as_str()) == caller.actor_integration_ref.as_deref()).unwrap();
        binding.actor_integration_params["cwr_action_actor_refs"] = json!([]);
    }
    assert_eq!(invoke("revoked", input.clone())["status"], 403,
        "revocation must reject new requests to the previously permitted gateway");
    assert_eq!(invoke("allowlisted", input.clone())["status"], 200,
        "a completed call can replay its cached result without running a gateway");
    assert_eq!(runtime.lock().unwrap().admission.invocation_records().count(), before);
    let mut state = runtime.lock().unwrap();
    let binding = state.graph.actor_integrations.iter_mut()
        .find(|binding| binding.actor_ref == caller.target_actor_ref
            && Some(binding.actor_integration_ref.as_str()) == caller.actor_integration_ref.as_deref()).unwrap();
    binding.actor_integration_params = original;
}

#[test]
fn gateway_tool_advertises_payload_without_requesting_host_identity() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = gateway_test_workspace(temp.path());
    let graph = crate::v2026_09::graph::resolve_graph(workspace).unwrap();
    let host_schema = graph.gateways[0].route.input_schema.clone();
    let operation = permitted_gateway_operation(&graph, None);
    let description = operation.input_schema["properties"]["request"]["description"].as_str().unwrap();
    let contracts: Value = serde_json::from_str(description.split_once("Permitted contracts: ").unwrap().1).unwrap();
    assert_eq!(contracts[0]["input_schema"], host_schema["properties"]["request"]);
    assert_eq!(graph.gateways[0].route.input_schema, host_schema);
    assert!(host_schema["required"].as_array().unwrap().contains(&json!("requester_actor_ref")));
    let public_input = json!({"type":GATEWAY_REQUEST_TYPE,"resource_ref":graph.gateways[0].resource_ref,"request":{"value":"synthetic"}});
    let validator = jsonschema::validator_for(&operation.input_schema).unwrap();
    assert!(validator.is_valid(&public_input));
    let mut forged = public_input;
    forged["requester_actor_ref"] = json!("cwp://fictional/actors/foreign");
    assert!(!validator.is_valid(&forged));
}

#[test]
fn gateway_payload_metadata_preserves_local_definition_references() {
    let schema = json!({"type":"object","$defs":{"Payload":{"type":"object","required":["operation"],"properties":{"operation":{"const":"folders"}}}},"properties":{"request":{"$ref":"#/$defs/Payload"}}});
    let projected = gateway_payload_schema(&schema);
    let validator = jsonschema::validator_for(&projected).unwrap();
    assert!(validator.is_valid(&json!({"operation":"folders"})));
    assert!(!validator.is_valid(&json!({"operation":"read"})));
    assert_eq!(schema["$defs"], projected["$defs"]);
}
