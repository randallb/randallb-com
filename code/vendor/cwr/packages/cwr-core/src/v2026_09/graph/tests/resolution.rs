use super::*;

#[test]
fn codex_role_toml_can_disable_selective_history() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './helper/COWORKER.toml'\nas = 'helper'\n",
    );
    person(
        dir,
        "helper",
        "[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = '../codex.toml'\nas = 'codex'\ndefault = true\n",
    );
    role(dir, "helper", "gpt-6-sol");
    integration(dir, "codex");
    let role_path = dir.join("helper/ROLE.toml");
    let role_text = fs::read_to_string(&role_path).unwrap().replace(
        "entrypoint = \"./PROMPT.md\"",
        "entrypoint = \"./PROMPT.md\"\nselective_history = false",
    );
    fs::write(role_path, role_text).unwrap();

    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    let binding = &graph.actor_integrations[0];
    assert_eq!(binding.adapter_params["selective_history"], false);
    assert!(binding.actor_integration_params["selective_history"].is_null());
}

#[test]
fn individual_resource_checks_keep_workspace_roots_and_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    person(
        dir,
        "bob",
        "[[imports]]\npath = '../shelly/COWORKER.toml'\nas = 'shell'\n",
    );
    person(dir, "shelly", "");
    root(
        dir,
        "[[imports]]\npath = './bob/COWORKER.toml'\nas = 'bob'\n",
    );
    let options = GraphLoadOptions {
        source_roots: vec![dir.to_owned()],
        ..GraphLoadOptions::default()
    };
    let source = resolve_resource_source_with_options(
        dir.join("CWR.toml"),
        "~@/bob",
        &EmptyResourceResolver,
        &options,
    )
    .unwrap();
    let checked = check_with_options(&source, &EmptyResourceResolver, &options).unwrap();
    assert_eq!(checked["actors"].as_array().unwrap().len(), 2);
    assert!(check(&source)
        .unwrap_err()
        .to_string()
        .contains("source roots"));
    let restricted = resolver::SourceResolver {
        host: &EmptyResourceResolver,
        roots: &options.source_roots,
    };
    assert!(check_with_resolver(&source, &restricted).is_ok());

    let outside = tempfile::tempdir().unwrap();
    person(outside.path(), "outside", "");
    std::os::unix::fs::symlink(outside.path(), dir.join("escape")).unwrap();
    person(
        dir,
        "bob",
        "[[imports]]\npath = '../escape/outside/COWORKER.toml'\n",
    );
    assert!(
        check_with_options(&source, &EmptyResourceResolver, &options)
            .unwrap_err()
            .to_string()
            .contains("source roots")
    );
}

#[test]
fn nested_exact_refs_resolve_after_all_path_branches_in_either_order() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    person(dir, "bob", "");
    person(dir, "alice", "");
    let bob =
        resolver::canonical_source_ref(&dir.join("bob/COWORKER.toml").canonicalize().unwrap());
    let alice =
        resolver::canonical_source_ref(&dir.join("alice/COWORKER.toml").canonicalize().unwrap());
    person(
        dir,
        "alice",
        &format!("[[imports]]\nref = {bob:?}\nas = 'bob'\n"),
    );
    person(
        dir,
        "bob",
        &format!("[[imports]]\nref = {alice:?}\nas = 'alice'\n"),
    );
    for order in [["alice", "bob"], ["bob", "alice"]] {
        root(
            dir,
            &order
                .into_iter()
                .map(|name| {
                    format!("[[imports]]\npath = './{name}/COWORKER.toml'\nas = '{name}'\n")
                })
                .collect::<String>(),
        );
        let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
        assert_eq!(graph.actors.len(), 2);
        assert_eq!(
            graph
                .actors
                .iter()
                .map(|actor| actor.actor_ref.clone())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([alice.clone(), bob.clone()])
        );
    }
}

#[test]
fn refs_to_later_exports_and_relationship_paths_converge() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    role(dir, "first", "gpt-5");
    integration(dir, "codex");
    let role_ref =
        resolver::canonical_source_ref(&dir.join("first/ROLE.toml").canonicalize().unwrap());
    person(dir, "alice", &format!("[[roles]]\nref = {role_ref:?}\n[[integrations]]\npath = '../codex.toml'\nas = 'codex'\n"));
    person(dir, "bob", "[[roles]]\npath = '../first/ROLE.toml'\n[[integrations]]\npath = '../codex.toml'\nas = 'codex'\n");
    let bob =
        resolver::canonical_source_ref(&dir.join("bob/COWORKER.toml").canonicalize().unwrap());
    write(
        dir,
        "team.toml",
        &format!("type = {ROOT_TYPE:?}\n[[exports]]\nref = {bob:?}\nas = 'bob'\n"),
    );
    root(dir, "[[imports]]\npath = './alice/COWORKER.toml'\n[[imports]]\npath = './team.toml'\nas = 'team'\n[[imports]]\nref = '~@/team/bob'\nas = 'selected'\n[[imports]]\npath = './bob/COWORKER.toml'\n");
    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    assert_eq!(graph.actors.len(), 2);
    assert!(graph
        .actors
        .iter()
        .all(|actor| actor.role_refs == vec![role_ref.clone()]));
    assert_eq!(
        resolve_resource_source(dir.join("CWR.toml"), "~@/selected").unwrap(),
        dir.join("bob/COWORKER.toml").canonicalize().unwrap()
    );
    root(dir, "[[imports]]\nref = 'cwp://com.example/unavailable'\n");
    assert!(resolve_graph(dir.join("CWR.toml"))
        .unwrap_err()
        .to_string()
        .contains("unresolved resource ref"));
}

#[test]
fn dependency_inputs_can_forward_a_property_literally_named_inputs() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    write(
        dir,
        "parent.schema.json",
        r#"{"type":"object","required":["parent_value"],"properties":{"parent_value":{"type":"string"}},"additionalProperties":false}"#,
    );
    write(
        dir,
        "child.schema.json",
        r#"{"type":"object","required":["inputs"],"properties":{"inputs":{"type":"string"}},"additionalProperties":false}"#,
    );
    root(dir, "[input_schema]\npath = './parent.schema.json'\n[[imports]]\npath = './child/COWORKER.toml'\n[imports.inputs]\ninputs = { input = 'parent_value' }\n");
    person(dir, "child", "[input_schema]\npath = '../child.schema.json'\n[[integrations]]\npath = '../provider.toml'\nas = 'provider'\n[integrations.inputs]\ninputs = { input = 'inputs' }\n[integrations.params.payload]\ninputs = { input = 'inputs' }\n");
    write(dir, "provider.toml", &format!("type = {INTEGRATION_TYPE:?}\n[input_schema]\npath = './child.schema.json'\n[adapter]\npath = './adapter.toml'\n[params.payload]\ninputs = {{ input = 'inputs' }}\n"));
    write(
        dir,
        "adapter.toml",
        &format!(
            "type = {ADAPTER_TYPE:?}\nref = 'cwp://com.example/forwarding'\nname = 'Forwarding'\n"
        ),
    );
    let options = GraphLoadOptions {
        inputs: json!({"parent_value": "forwarded value"}),
        ..GraphLoadOptions::default()
    };
    let graph =
        resolve_graph_with_options(dir.join("CWR.toml"), &EmptyResourceResolver, &options).unwrap();
    let binding = &graph.actor_integrations[0];
    assert_eq!(binding.adapter_name, "Forwarding");
    assert_eq!(
        binding.actor_integration_params["payload"]["inputs"],
        "forwarded value"
    );
    assert_eq!(
        binding.integration_params["payload"]["inputs"],
        "forwarded value"
    );
}

#[test]
fn bindings_without_roles_must_satisfy_the_adapter_contract() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    person(
        dir,
        "bob",
        "[[integrations]]\npath = '../provider.toml'\nas = 'provider'\n",
    );
    root(dir, "[[imports]]\npath = './bob/COWORKER.toml'\n");
    for provider in ["codex", "claude", "open-responses", "sh"] {
        let params = if provider == "open-responses" {
            "[params]\nendpoint = 'http://localhost/responses'\n"
        } else {
            ""
        };
        write(dir, "provider.toml", &format!("type = {INTEGRATION_TYPE:?}\n{params}[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/{provider}'\n"));
        let result = resolve_graph(dir.join("CWR.toml"));
        if provider == "sh" {
            assert_eq!(result.unwrap().actor_integrations.len(), 1);
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("composed coworker parameters"),
                "{provider}"
            );
        }
    }
}

#[test]
fn composed_role_parameters_are_validated_as_one_adapter_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    write(dir, "adapter.toml", &format!("type = {ADAPTER_TYPE:?}\nref = 'cwp://com.example/adapter'\nname = 'Exclusive parameters'\n[coworker_params_schema]\npath = './params.schema.json'\n"));
    write(
        dir,
        "params.schema.json",
        r#"{"type":"object","maxProperties":1,"properties":{"one":{"type":"string"},"two":{"type":"string"}},"additionalProperties":false}"#,
    );
    for name in ["one", "two"] {
        write(dir, &format!("{name}/ROLE.toml"), &format!("type = {ROLE_TYPE:?}\n[[adapters]]\npath = '../adapter.toml'\n[adapters.params]\n{name} = 'value'\n"));
    }
    write(
        dir,
        "provider.toml",
        &format!("type = {INTEGRATION_TYPE:?}\n[adapter]\npath = './adapter.toml'\n"),
    );
    person(dir, "bob", "[[roles]]\npath = '../one/ROLE.toml'\n[[roles]]\npath = '../two/ROLE.toml'\n[[integrations]]\npath = '../provider.toml'\nas = 'provider'\n");
    root(dir, "[[imports]]\npath = './bob/COWORKER.toml'\n");
    let error = resolve_graph(dir.join("CWR.toml")).unwrap_err().to_string();
    assert!(error.contains("composed coworker parameters"), "{error}");
    person(dir, "bob", "[[roles]]\npath = '../one/ROLE.toml'\n[[integrations]]\npath = '../provider.toml'\nas = 'provider'\n");
    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    assert_eq!(graph.actor_integrations[0].adapter_params["one"], "value");
}
