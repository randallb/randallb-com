use super::*;

#[test]
fn nested_volume_runtimes_keep_their_coworkers_separate() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './one/CWR.toml'\nas = 'one'\n");
    write(dir, "one/CWR.toml", &format!(
        "type = {RUNTIME_TYPE:?}\n[[volumes]]\nas = 'memory'\naccess = 'read'\n[[imports]]\npath = './helper/COWORKER.toml'\nas = 'helper'\n[[imports]]\npath = '../two/CWR.toml'\nas = 'two'\n"
    ));
    write(dir, "two/CWR.toml", &format!(
        "type = {RUNTIME_TYPE:?}\n[[volumes]]\nas = 'memory'\naccess = 'read'\n[[imports]]\npath = './helper/COWORKER.toml'\nas = 'helper'\n"
    ));
    for name in ["one", "two"] {
        write(
            dir,
            &format!("{name}/helper/COWORKER.toml"),
            &format!("type = {COWORKER_TYPE:?}\nname = '{name} helper'\nslug = '{name}_helper'\n"),
        );
        fs::create_dir(dir.join(format!("{name}-memory"))).unwrap();
    }
    let mut options = GraphLoadOptions::default();
    for name in ["one", "two"] {
        options.imported_volumes.push(ImportedVolumeBinding {
            source: dir.join(name).join("CWR.toml"),
            alias: "memory".into(),
            path: dir.join(format!("{name}-memory")),
            access: "read".into(),
        });
    }
    resolve_graph_with_options(dir.join("CWR.toml"), &EmptyResourceResolver, &options).unwrap();
}

#[test]
fn distinct_coworkers_may_share_a_role_across_volume_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './one/CWR.toml'\nas = 'one'\n[[imports]]\npath = './two/CWR.toml'\nas = 'two'\n");
    role(dir, "shared", "gpt-6-sol");
    for name in ["one", "two"] {
        let volume = if name == "one" {
            "[[volumes]]\nas = 'memory'\naccess = 'read'\n"
        } else {
            ""
        };
        write(dir, &format!("{name}/CWR.toml"), &format!(
            "type = {RUNTIME_TYPE:?}\n{volume}[[imports]]\npath = './helper/COWORKER.toml'\nas = 'helper'\n"
        ));
        write(dir, &format!("{name}/helper/COWORKER.toml"), &format!(
            "type = {COWORKER_TYPE:?}\nname = '{name} helper'\nslug = '{name}_helper'\n[[roles]]\npath = '../../shared/ROLE.toml'\n"
        ));
    }
    fs::create_dir(dir.join("one-memory")).unwrap();
    let mut options = GraphLoadOptions::default();
    options.imported_volumes.push(ImportedVolumeBinding {
        source: dir.join("one/CWR.toml"),
        alias: "memory".into(),
        path: dir.join("one-memory"),
        access: "read".into(),
    });
    resolve_graph_with_options(dir.join("CWR.toml"), &EmptyResourceResolver, &options).unwrap();
}

#[test]
fn standalone_shell_gateway_handler_receives_its_runtime_volume_mapping() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './runtime/CWR.toml'\nas = 'runtime'\n",
    );
    write(
        dir,
        "runtime/CWR.toml",
        &format!(
            "type = {RUNTIME_TYPE:?}\n[[volumes]]\nas = 'memory'\naccess = 'read_write'\n[[imports]]\npath = './gateway/GATEWAY.toml'\nas = 'gateway'\n"
        ),
    );
    write(
        dir,
        "runtime/gateway/GATEWAY.toml",
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/gateway'\nrole = { ref = '~@/handler' }\nintegration = { ref = '~@/provider' }\naction_name = 'handle--request'\n[[imports]]\npath = './ROLE.toml'\nas = 'handler'\n[[imports]]\npath = './integration.toml'\nas = 'provider'\n",
    );
    write(
        dir,
        "runtime/gateway/ROLE.toml",
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n[[actions]]\nverb = 'handle'\nobject = 'request'\ntitle = 'Handle request'\ndescription = 'Handle a gateway request'\n[actions.input_schema]\npath = './schema.json'\n[actions.output_schema]\npath = './schema.json'\n",
    );
    write(
        dir,
        "runtime/gateway/PROMPT.md",
        "Use the configured memory volume.",
    );
    write(dir, "runtime/gateway/schema.json", r#"{"type":"object"}"#);
    write(
        dir,
        "runtime/gateway/integration.toml",
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n",
    );
    fs::create_dir(dir.join("memory")).unwrap();
    let mut options = GraphLoadOptions::default();
    options.imported_volumes.push(ImportedVolumeBinding {
        source: dir.join("runtime/CWR.toml"),
        alias: "memory".into(),
        path: dir.join("memory"),
        access: "read_write".into(),
    });
    let graph =
        resolve_graph_with_options(dir.join("CWR.toml"), &EmptyResourceResolver, &options).unwrap();
    let gateway = graph.gateways.first().unwrap();
    assert_eq!(
        gateway.route.actor_integration_params["activated_volumes"]["memory"]["path"],
        dir.join("memory")
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        gateway.route.actor_integration_params["activated_volumes"]["memory"]["access"],
        "read_write"
    );
    assert!(gateway
        .route
        .adapter_params
        .get("activated_volumes")
        .is_none());
    let activated = gateway.route.adapter_params["activated_sources"]
        .as_object()
        .unwrap();
    let instructions = activated
        .values()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        instructions.contains("memory (read_write)"),
        "{instructions}"
    );
    assert!(instructions.contains(&dir.join("memory").to_string_lossy().to_string()));
}

#[test]
fn imported_volume_bindings_are_scoped_to_each_runtime_and_its_coworkers() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './one/CWR.toml'\nas = 'one'\n[[imports]]\npath = './two/CWR.toml'\nas = 'two'\n",
    );
    for name in ["one", "two"] {
        let actor = format!("{name}_helper");
        write(
            dir,
            &format!("{name}/CWR.toml"),
            &format!(
                "type = {RUNTIME_TYPE:?}\n[[volumes]]\nas = 'memory'\naccess = 'read_write'\n[[imports]]\npath = './{actor}/COWORKER.toml'\nas = 'helper'\n"
            ),
        );
        person(
            &dir.join(name),
            &actor,
            "[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = '../codex.toml'\nas = 'codex'\ndefault = true\n",
        );
        role(&dir.join(name), &actor, "gpt-6-sol");
        integration(&dir.join(name), "codex");
        fs::create_dir(dir.join(format!("{name}-memory"))).unwrap();
    }
    let manifest = dir.join("CWR.toml");
    let missing = resolve_graph(&manifest).unwrap_err().to_string();
    assert!(missing.contains("explicit folder binding"), "{missing}");
    let mut options = GraphLoadOptions::default();
    for name in ["one", "two"] {
        options.imported_volumes.push(ImportedVolumeBinding {
            source: dir.join(name).join("CWR.toml"),
            alias: "memory".into(),
            path: dir.join(format!("{name}-memory")),
            access: "read_write".into(),
        });
    }
    let graph = resolve_graph_with_options(&manifest, &EmptyResourceResolver, &options).unwrap();
    assert_eq!(graph.actor_integrations.len(), 2);
    let canonical = dir.canonicalize().unwrap();
    for binding in &graph.actor_integrations {
        let name = if binding.coworker_source.starts_with(canonical.join("one")) {
            "one"
        } else {
            "two"
        };
        let instructions = binding.adapter_params["activated_sources"]
            .as_object()
            .unwrap()
            .values()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            instructions.contains(&format!("{name}-memory")),
            "{name}: {instructions}"
        );
        assert!(!instructions.contains(&format!(
            "{}-memory",
            if name == "one" { "two" } else { "one" }
        )));
    }
    options.imported_volumes[0].access = "read".into();
    assert!(
        resolve_graph_with_options(&manifest, &EmptyResourceResolver, &options)
            .unwrap_err()
            .to_string()
            .contains("mismatched access")
    );
    options.imported_volumes[0].access = "read_write".into();
    options.imported_volumes[0].source = dir.join("unknown.toml");
    assert!(resolve_graph_with_options(&manifest, &EmptyResourceResolver, &options).is_err());
}

#[test]
fn external_coworkers_follow_import_ownership_and_shared_ones_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './one/CWR.toml'\nas = 'one'\n");
    person(
        dir,
        "shared",
        "[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = '../codex.toml'\nas = 'codex'\ndefault = true\n",
    );
    role(dir, "shared", "gpt-6-sol");
    integration(dir, "codex");
    for name in ["one", "two"] {
        write(
            dir,
            &format!("{name}/CWR.toml"),
            &format!(
                "type = {RUNTIME_TYPE:?}\n[[volumes]]\nas = 'memory'\naccess = 'read'\n[[imports]]\npath = '../shared/COWORKER.toml'\nas = 'shared'\n"
            ),
        );
        fs::create_dir(dir.join(format!("{name}-memory"))).unwrap();
    }
    let mut options = GraphLoadOptions::default();
    options.imported_volumes.push(ImportedVolumeBinding {
        source: dir.join("one/CWR.toml"),
        alias: "memory".into(),
        path: dir.join("one-memory"),
        access: "read".into(),
    });
    let manifest = dir.join("CWR.toml");
    let graph = resolve_graph_with_options(&manifest, &EmptyResourceResolver, &options).unwrap();
    let instructions = graph.actor_integrations[0].adapter_params["activated_sources"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(instructions.contains("memory (read)"));
    assert!(instructions.contains("one-memory"));
    root(
        dir,
        "[[imports]]\npath = './one/CWR.toml'\nas = 'one'\n[[imports]]\npath = './two/CWR.toml'\nas = 'two'\n",
    );
    options.imported_volumes.push(ImportedVolumeBinding {
        source: dir.join("two/CWR.toml"),
        alias: "memory".into(),
        path: dir.join("two-memory"),
        access: "read".into(),
    });
    assert!(
        resolve_graph_with_options(&manifest, &EmptyResourceResolver, &options)
            .unwrap_err()
            .to_string()
            .contains("shares a resource outside its imports")
    );
    write(
        dir,
        "two/CWR.toml",
        &format!(
            "type = {RUNTIME_TYPE:?}\n[[imports]]\npath = '../shared/COWORKER.toml'\nas = 'shared'\n"
        ),
    );
    options.imported_volumes.pop();
    assert!(
        resolve_graph_with_options(&manifest, &EmptyResourceResolver, &options)
            .unwrap_err()
            .to_string()
            .contains("shares a resource outside its imports")
    );
}

#[test]
fn volume_owner_cannot_share_an_intermediate_import() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './one/CWR.toml'\nas = 'one'\n[[imports]]\npath = './shared/CWR.toml'\nas = 'shared'\n",
    );
    write(
        dir,
        "one/CWR.toml",
        &format!(
            "type = {RUNTIME_TYPE:?}\n[[volumes]]\nas = 'memory'\naccess = 'read'\n[[imports]]\npath = '../shared/CWR.toml'\nas = 'shared'\n"
        ),
    );
    write(
        dir,
        "shared/CWR.toml",
        &format!("type = {RUNTIME_TYPE:?}\n[[imports]]\npath = './COWORKER.toml'\nas = 'person'\n"),
    );
    person(
        dir,
        "shared",
        "[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = '../codex.toml'\nas = 'codex'\ndefault = true\n",
    );
    role(dir, "shared", "gpt-6-sol");
    integration(dir, "codex");
    fs::create_dir(dir.join("memory")).unwrap();
    let mut options = GraphLoadOptions::default();
    options.imported_volumes.push(ImportedVolumeBinding {
        source: dir.join("one/CWR.toml"),
        alias: "memory".into(),
        path: dir.join("memory"),
        access: "read".into(),
    });
    assert!(
        resolve_graph_with_options(dir.join("CWR.toml"), &EmptyResourceResolver, &options,)
            .unwrap_err()
            .to_string()
            .contains("shares a resource outside its imports")
    );
}
