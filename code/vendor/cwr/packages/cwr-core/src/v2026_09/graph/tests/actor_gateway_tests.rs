use super::*;

#[test]
fn human_readable_names_work_without_authoring_a_tool_slug() {
    let temp = tempfile::tempdir().unwrap();
    root(temp.path(), "[[imports]]\npath = './COWORKER.toml'\n");
    write(
        temp.path(),
        "COWORKER.toml",
        &format!("type = {COWORKER_TYPE:?}\nname = 'Photo Editor'\n"),
    );
    let graph = resolve_graph(temp.path().join("CWR.toml")).unwrap();
    assert_eq!(graph.actors[0].coworker_slug, "photo_editor");
}

#[test]
fn configured_action_permissions_resolve_in_the_coworkers_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './bob/COWORKER.toml'\n");
    person(dir, "shelly", "");
    role(dir, "first", "gpt-5");
    integration(dir, "codex");
    person(
        dir,
        "bob",
        "[[imports]]\npath = '../shelly/COWORKER.toml'\nas = 'shell'\n[[roles]]\npath = '../first/ROLE.toml'\n[[integrations]]\npath = '../codex.toml'\nas = 'codex'\n[integrations.params]\ncwr_action_actor_refs = ['~@/shell']\n",
    );
    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    let shelly = graph
        .actors
        .iter()
        .find(|actor| actor.coworker_slug == "shelly")
        .unwrap();
    assert_eq!(
        graph.actor_integrations[0].actor_integration_params["cwr_action_actor_refs"],
        json!([shelly.actor_ref])
    );
}

#[test]
fn open_responses_action_permissions_resolve_in_the_coworkers_namespace() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './bob/COWORKER.toml'\n");
    person(dir, "shelly", "");
    write(
        dir,
        "first/ROLE.toml",
        &format!(
            "type = {ROLE_TYPE:?}\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/open-responses'\n[adapters.params]\nentrypoint = './PROMPT.md'\nmodel = 'local-model'\n"
        ),
    );
    write(dir, "first/PROMPT.md", "Use only declared CWR actions.");
    write(
        dir,
        "ollama.toml",
        &format!(
            "type = {INTEGRATION_TYPE:?}\n[params]\nendpoint = 'http://127.0.0.1:11434/v1/responses'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/open-responses'\n"
        ),
    );
    person(
        dir,
        "bob",
        "[[imports]]\npath = '../shelly/COWORKER.toml'\nas = 'shell'\n[[roles]]\npath = '../first/ROLE.toml'\n[[integrations]]\npath = '../ollama.toml'\nas = 'ollama'\n[integrations.params]\ncwr_action_actor_refs = ['~@/shell']\n",
    );
    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    let shelly = graph
        .actors
        .iter()
        .find(|actor| actor.coworker_slug == "shelly")
        .unwrap();
    assert_eq!(
        graph.actor_integrations[0].actor_integration_params["cwr_action_actor_refs"],
        json!([shelly.actor_ref])
    );
}

#[test]
fn message_preference_requires_known_distinct_conversational_bindings_default_first() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './bob/COWORKER.toml'\n");
    role(dir, "first", "gpt-5");
    integration(dir, "one");
    integration(dir, "two");
    let declarations = "[[roles]]\npath = '../first/ROLE.toml'\n[[integrations]]\npath = '../one.toml'\nas = 'one'\ndefault = true\n[[integrations]]\npath = '../two.toml'\nas = 'two'\n";
    for (preference, valid) in [
        ("['one','two']", true),
        ("['two','one']", false),
        ("['one','missing']", false),
        ("['one','one']", false),
        ("[]", false),
    ] {
        person(
            dir,
            "bob",
            &format!("message_binding_preference = {preference}\n{declarations}"),
        );
        let result = resolve_graph(dir.join("CWR.toml"));
        assert_eq!(result.is_ok(), valid, "{preference}: {result:?}");
    }
    person(
        dir,
        "bob",
        &format!(
            "message_binding_preference = ['one', 'two']\n{}",
            declarations.replace("as = 'two'", "as = 'two'\nmessage_mode = 'actions_only'")
        ),
    );
    assert!(resolve_graph(dir.join("CWR.toml")).is_err());
}

#[test]
fn standalone_gateway_resolves_its_role_and_integration_without_a_coworker() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './gateway/GATEWAY.toml'\nas = 'google'\n",
    );
    let gateway_dir = dir.join("gateway");
    role(&gateway_dir, "handler", "gpt-6-sol");
    integration(&gateway_dir, "provider");
    write(
        &gateway_dir,
        "GATEWAY.toml",
        &format!(
            r#"type = {GATEWAY_TYPE:?}
role = {{ ref = "~@/handler" }}
integration = {{ ref = "~@/provider" }}
action_name = "make--handler"

[[imports]]
path = "./handler/ROLE.toml"
as = "handler"

[[imports]]
path = "./provider.toml"
as = "provider"
"#
        ),
    );

    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    assert!(graph.actors.is_empty());
    assert!(graph.actor_integrations.is_empty());
    assert!(graph.routes.is_empty());
    assert_eq!(graph.gateways.len(), 1);
    assert_eq!(
        check(gateway_dir.join("GATEWAY.toml")).unwrap()["gateways"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let path = gateway_dir.join("GATEWAY.toml");
    let original = fs::read_to_string(&path).unwrap();
    for replacement in [
        original.replace("make--handler", "missing--action"),
        original.replace("~@/handler", "~@/missing"),
        format!("{original}\n[params]\nmodel = 42\n"),
        original.replace(
            "action_name = \"make--handler\"",
            "authority = ['cwr:configure']\naction_name = \"make--handler\"",
        ),
    ] {
        fs::write(&path, replacement).unwrap();
        assert!(check(&path).is_err());
    }
    fs::write(&path, original).unwrap();
    let role_path = gateway_dir.join("handler/ROLE.toml");
    let role_text = fs::read_to_string(&role_path).unwrap();
    fs::write(
        &role_path,
        role_text.replace(
            "verb = \"make\"",
            "response_mode = 'async'\nverb = \"make\"",
        ),
    )
    .unwrap();
    assert!(check(&path)
        .unwrap_err()
        .to_string()
        .contains("synchronous"));
    fs::write(role_path, role_text).unwrap();
    assert!(graph.gateways[0]
        .request_refs
        .contains(&format!("{}/google", graph.reference)));
    assert_eq!(graph.gateways[0].route.action_name, "make--handler");
    assert_eq!(
        graph.gateways[0].route.coworker_ref,
        graph.gateways[0].resource_ref
    );
    assert_eq!(
        graph.gateways[0].route.actor_ref,
        graph.gateways[0].resource_ref
    );
}

#[test]
fn standalone_gateway_schema_refs_stay_within_source_roots() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './gateway/GATEWAY.toml'\nas = 'gateway'\n",
    );
    let gateway_dir = dir.join("gateway");
    role(&gateway_dir, "handler", "gpt-6-sol");
    integration(&gateway_dir, "provider");
    let role_path = gateway_dir.join("handler/ROLE.toml");
    let role_text = fs::read_to_string(&role_path).unwrap().replace(
        "path = \"./schema.json\"",
        "ref = \"cwp://example.test/schema\"",
    );
    fs::write(role_path, role_text).unwrap();
    write(
        &gateway_dir,
        "GATEWAY.toml",
        &format!("type = {GATEWAY_TYPE:?}\nrole = {{ ref = '~@/handler' }}\nintegration = {{ ref = '~@/provider' }}\naction_name = 'make--handler'\n[[imports]]\npath = './handler/ROLE.toml'\nas = 'handler'\n[[imports]]\npath = './provider.toml'\nas = 'provider'\n"),
    );
    write(outside.path(), "schema.json", r#"{"type":"object"}"#);
    struct Host(PathBuf);
    impl CheckResourceResolver for Host {
        fn resolve(&self, reference: &str) -> CwrResult<Option<PathBuf>> {
            Ok((reference == "cwp://example.test/schema").then(|| self.0.clone()))
        }
    }
    let error = resolve_graph_with_resolver(
        dir.join("CWR.toml"),
        &Host(outside.path().join("schema.json")),
    )
    .unwrap_err();
    assert!(error.to_string().contains("source roots"), "{error}");
}

#[test]
fn standalone_gateway_forwards_declared_inputs_to_its_integration() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[inputs]]\ntype = 'cwp://org.coworkerprotocol/2026-09/schemas/plaintext'\nas = 'api_token'\n[[imports]]\npath = './gateway/GATEWAY.toml'\nas = 'gateway'\n[imports.inputs]\napi_token = { input = 'api_token' }\n",
    );
    let gateway = dir.join("gateway");
    write(
        &gateway,
        "GATEWAY.toml",
        &format!(
            "type = {GATEWAY_TYPE:?}\nrole = {{ ref = '~@/handler' }}\nintegration = {{ ref = '~@/provider' }}\naction_name = 'handle--request'\n[[imports]]\npath = './ROLE.toml'\nas = 'handler'\n[[imports]]\npath = './integration.toml'\nas = 'provider'\n[imports.inputs]\ntoken = {{ input = 'api_token' }}\n[[inputs]]\ntype = \"cwp://org.coworkerprotocol/2026-09/schemas/plaintext\"\nas = 'api_token'\n"
        ),
    );
    write(
        &gateway,
        "ROLE.toml",
        &format!("type = {ROLE_TYPE:?}\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n[[actions]]\nverb = 'handle'\nobject = 'request'\ntitle = 'Handle request'\ndescription = 'Handle a request'\n[actions.input_schema]\npath = './schema.json'\n[actions.output_schema]\npath = './schema.json'\n"),
    );
    write(&gateway, "schema.json", r#"{"type":"object"}"#);
    write(
        &gateway,
        "integration.toml",
        &format!(
            "type = {INTEGRATION_TYPE:?}\n[[inputs]]\ntype = \"cwp://org.coworkerprotocol/2026-09/schemas/plaintext\"\nas = 'token'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n[params.env]\nAPI_TOKEN = {{ input = 'token' }}\n"
        ),
    );
    let options = GraphLoadOptions {
        inputs: json!({"api_token": "gateway-input-sentinel"}),
        ..GraphLoadOptions::default()
    };
    let graph =
        resolve_graph_with_options(dir.join("CWR.toml"), &EmptyResourceResolver, &options).unwrap();
    assert_eq!(graph.gateways.len(), 1);
    assert_eq!(
        graph.gateways[0].route.integration_params["env"]["API_TOKEN"],
        "gateway-input-sentinel"
    );
}

#[test]
fn standalone_gateway_resolves_absolute_relationships_and_their_inputs() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    role(dir, "handler", "gpt-6-sol");
    integration(dir, "provider");
    let provider = dir.join("provider.toml");
    fs::write(&provider, format!("{}\n[[inputs]]\ntype = 'cwp://org.coworkerprotocol/2026-09/schemas/plaintext'\nas = 'token'\n", fs::read_to_string(&provider).unwrap())).unwrap();
    let role_path = dir.join("handler/ROLE.toml");
    fs::write(&role_path, format!("{}\n[[inputs]]\ntype = 'cwp://org.coworkerprotocol/2026-09/schemas/plaintext'\nas = 'title'\n", fs::read_to_string(&role_path).unwrap().replace("model = \"gpt-6-sol\"", "model = { input = 'title' }"))).unwrap();
    write(dir, "GATEWAY.toml", &format!("type = {GATEWAY_TYPE:?}\nrole = {{ ref = 'cwp://example.test/role', inputs = {{ title = 'gpt-6-sol' }} }}\nintegration = {{ ref = 'cwp://example.test/integration', inputs = {{ token = 'gpt-6-sol' }} }}\naction_name = 'make--handler'\n"));
    root(dir, "[[imports]]\npath = './GATEWAY.toml'\n");
    struct Host(PathBuf);
    impl CheckResourceResolver for Host {
        fn resolve(&self, reference: &str) -> CwrResult<Option<PathBuf>> {
            Ok(match reference {
                "cwp://example.test/role" => Some(self.0.join("handler/ROLE.toml")),
                "cwp://example.test/integration" => Some(self.0.join("provider.toml")),
                _ => None,
            })
        }
    }
    let host = Host(dir.to_owned());
    let graph = resolve_graph_with_resolver(dir.join("CWR.toml"), &host).unwrap();
    assert_eq!(graph.gateways.len(), 1);
    assert_eq!(graph.gateways[0].route.adapter_params["model"], "gpt-6-sol");
    assert!(check_with_resolver(dir.join("GATEWAY.toml"), &host).is_ok());
}

#[test]
fn restricted_caller_can_select_a_standalone_gateway() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    role(dir, "handler", "gpt-6-sol");
    integration(dir, "provider");
    write(dir, "GATEWAY.toml", &format!("type = {GATEWAY_TYPE:?}\nrole = {{ ref = '~@/handler' }}\nintegration = {{ ref = '~@/provider' }}\naction_name = 'make--handler'\n[[imports]]\npath = './handler/ROLE.toml'\nas = 'handler'\n[[imports]]\npath = './provider.toml'\nas = 'provider'\n"));
    person(dir, "bob", "[[imports]]\npath = '../GATEWAY.toml'\nas = 'gateway'\n[[roles]]\npath = '../handler/ROLE.toml'\n[[integrations]]\npath = '../provider.toml'\nas = 'provider'\n[integrations.params]\ncwr_action_actor_refs = ['~@/gateway']\n");
    root(dir, "[[imports]]\npath = './bob/COWORKER.toml'\n");
    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    assert_eq!(
        graph.actor_integrations[0].actor_integration_params["cwr_action_actor_refs"],
        json!([graph.gateways[0].resource_ref])
    );
}
