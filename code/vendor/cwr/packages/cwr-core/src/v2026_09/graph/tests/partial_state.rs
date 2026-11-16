use super::*;

fn partial_error(dir: &Path) -> String {
    resolve_graph_with_options(
        dir.join("CWR.toml"),
        &EmptyResourceResolver,
        &GraphLoadOptions {
            allow_partial: true,
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string()
}

#[test]
fn malformed_shared_state_imports_remain_fatal() {
    for type_ref in [CONVERSATION_TYPE, ACTION_INVOCATION_TYPE] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        root(
            dir,
            "[[imports]]\npath = './good/COWORKER.toml'\n[[imports]]\npath = './state.toml'\n",
        );
        helper(dir, "good");
        write(
            dir,
            "state.toml",
            &format!("type = {type_ref:?}\nref = 'cwp://example.test/state'\n"),
        );
        assert!(partial_error(dir).contains("state.toml"));
    }
}

#[test]
fn shared_conversations_reject_coworkers_removed_during_projection() {
    for failure in ["invalid_role", "invalid_binding", "collision"] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        root(dir, "[[imports]]\npath = './good/COWORKER.toml'\n[[imports]]\npath = './bad/COWORKER.toml'\n[[imports]]\npath = './state.toml'\n");
        helper(dir, "good");
        helper(dir, "bad");
        match failure {
            "invalid_role" => write(dir, "bad/schema.json", "invalid JSON"),
            "invalid_binding" => write(dir, "bad.toml", &format!("type = {INTEGRATION_TYPE:?}\nparams = 42\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/codex'\n")),
            "collision" => {
                let path = dir.join("bad/COWORKER.toml");
                let text = fs::read_to_string(&path).unwrap().replace("name = \"bad\"", "name = \"good\"").replace("slug = \"bad\"", "slug = \"good\"");
                fs::write(path, text).unwrap();
            }
            _ => unreachable!(),
        }
        let actor =
            resolver::canonical_source_ref(&dir.join("bad/COWORKER.toml").canonicalize().unwrap());
        write(dir, "state.toml", &format!("type = {CONVERSATION_TYPE:?}\nref = 'cwp://example.test/conversation'\nparticipant_actor_refs = [{actor:?}]\n"));
        assert!(
            partial_error(dir).contains("shared state names unavailable coworker"),
            "{failure}"
        );
    }
}

#[test]
fn invalid_pending_binding_inputs_leave_healthy_bindings_usable() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './person/COWORKER.toml'\n");
    helper(dir, "person");
    integration(dir, "bad");
    integration(dir, "host");
    person(dir, "person", "[[roles]]\npath = './ROLE.toml'\n[[imports]]\npath = '../bad.toml'\nas = 'bad'\n[[integrations]]\nref = '~@/bad'\nas = 'bad'\ndefault = true\n[integrations.inputs]\nprofile = { input = 'unknown' }\n[[integrations]]\npath = '../host.toml'\nas = 'host'\n");
    let graph = partial(dir);
    assert_eq!(names(&graph), ["person"]);
    assert_eq!(graph.actor_integrations.len(), 1);
    assert!(graph.actor_integrations[0]
        .actor_integration_ref
        .ends_with("/host"));
    assert_eq!(graph.routes.len(), 1);
    assert!(resolve_graph(dir.join("CWR.toml")).is_err());
}

#[test]
fn invalid_pending_role_adapter_inputs_disable_only_that_adapter() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './person/COWORKER.toml'\n");
    helper(dir, "person");
    let path = dir.join("person/ROLE.toml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{text}\n[adapters.inputs]\nprofile = {{ input = 'unknown' }}\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/claude'\n[adapters.params]\nentrypoint = './PROMPT.md'\nmodel = 'test-claude'\n")).unwrap();
    write(dir, "claude.toml", &format!("type = {INTEGRATION_TYPE:?}\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/claude'\n"));
    let path = dir.join("person/COWORKER.toml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!("{text}\n[[integrations]]\npath = '../claude.toml'\nas = 'claude'\n"),
    )
    .unwrap();
    let graph = partial(dir);
    assert_eq!(names(&graph), ["person"]);
    assert_eq!(graph.actor_integrations.len(), 1);
    assert_eq!(
        graph.actor_integrations[0].adapter_ref,
        crate::adapter::CLAUDE_ADAPTER_REF
    );
    assert_eq!(graph.routes.len(), 1);
    assert!(resolve_graph(dir.join("CWR.toml")).is_err());
}

#[test]
fn disabled_import_does_not_require_its_saved_volume_grant() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './good/COWORKER.toml'\n[[imports]]\npath = './broken/CWR.toml'\n",
    );
    helper(dir, "good");
    write(
        dir,
        "broken/CWR.toml",
        &format!("type = {RUNTIME_TYPE:?}\nunknown = true\n"),
    );
    fs::create_dir(dir.join("memory")).unwrap();
    let options = GraphLoadOptions {
        allow_partial: true,
        imported_volumes: vec![ImportedVolumeBinding {
            source: dir.join("broken/CWR.toml"),
            alias: "memory".into(),
            path: dir.join("memory"),
            access: "read".into(),
        }],
        ..Default::default()
    };
    let graph =
        resolve_graph_with_options(dir.join("CWR.toml"), &EmptyResourceResolver, &options).unwrap();
    assert_eq!(names(&graph), ["good"]);
    assert_eq!(graph.routes.len(), 1);
    assert!(!graph.warnings.is_empty());
    let strict = GraphLoadOptions {
        allow_partial: false,
        ..options
    };
    assert!(
        resolve_graph_with_options(dir.join("CWR.toml"), &EmptyResourceResolver, &strict).is_err()
    );
}

#[test]
fn disabled_integration_adapter_leaves_other_bindings_usable() {
    for adapter in [
        "path = './missing-adapter.toml'",
        "path = './invalid-adapter.toml'",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        root(dir, "[[imports]]\npath = './person/COWORKER.toml'\n");
        helper(dir, "person");
        integration(dir, "host");
        write(
            dir,
            "person.toml",
            &format!("type = {INTEGRATION_TYPE:?}\n[adapter]\n{adapter}\n"),
        );
        write(dir, "invalid-adapter.toml", "not valid TOML");
        let path = dir.join("person/COWORKER.toml");
        let text = fs::read_to_string(&path).unwrap();
        fs::write(
            path,
            format!("{text}\n[[integrations]]\npath = '../host.toml'\nas = 'host'\n"),
        )
        .unwrap();
        let graph = partial(dir);
        assert_eq!(names(&graph), ["person"]);
        assert_eq!(graph.actor_integrations.len(), 1);
        assert!(graph.actor_integrations[0]
            .actor_integration_ref
            .ends_with("/host"));
        assert_eq!(graph.routes.len(), 1);
        assert!(resolve_graph(dir.join("CWR.toml")).is_err());
    }
}

#[test]
fn invalid_path_receiver_inputs_do_not_disable_a_shared_integration() {
    for invalid_first in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let imports = if invalid_first {
            ["bad", "good"]
        } else {
            ["good", "bad"]
        };
        root(
            dir,
            &imports
                .iter()
                .map(|name| format!("[[imports]]\npath = './{name}/COWORKER.toml'\n"))
                .collect::<String>(),
        );
        for name in ["good", "bad"] {
            helper(dir, name);
            person(dir, name, &format!("[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = '../shared.toml'\nas = 'shared'\ndefault = true\n{}", if name == "good" { "[integrations.inputs]\nprofile = 'valid'\n" } else { "" }));
        }
        write(dir, "shared.toml", &format!("type = {INTEGRATION_TYPE:?}\n[[inputs]]\nas = 'profile'\ntype = 'cwp://org.coworkerprotocol/2026-09/schemas/plaintext'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/codex'\n"));
        let graph = partial(dir);
        assert_eq!(names(&graph), ["good"]);
        assert_eq!(graph.routes.len(), 1);
        assert_eq!(graph.actor_integrations.len(), 1);
        assert!(resolve_graph(dir.join("CWR.toml")).is_err());
    }
}

#[test]
fn receiver_failed_runtime_does_not_require_its_saved_volume_grant() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './good/COWORKER.toml'\n[[imports]]\npath = './imported/CWR.toml'\n",
    );
    helper(dir, "good");
    write(dir, "imported/CWR.toml", &format!("type = {RUNTIME_TYPE:?}\n[[inputs]]\nas = 'profile'\ntype = 'cwp://org.coworkerprotocol/2026-09/schemas/plaintext'\n[[volumes]]\nas = 'memory'\naccess = 'read'\n"));
    fs::create_dir(dir.join("memory")).unwrap();
    let options = GraphLoadOptions {
        allow_partial: true,
        imported_volumes: vec![ImportedVolumeBinding {
            source: dir.join("imported/CWR.toml"),
            alias: "memory".into(),
            path: dir.join("memory"),
            access: "read".into(),
        }],
        ..Default::default()
    };
    let graph =
        resolve_graph_with_options(dir.join("CWR.toml"), &EmptyResourceResolver, &options).unwrap();
    assert_eq!(names(&graph), ["good"]);
    assert_eq!(graph.routes.len(), 1);
    assert!(!graph.warnings.is_empty());
    let strict = GraphLoadOptions {
        allow_partial: false,
        ..options
    };
    assert!(
        resolve_graph_with_options(dir.join("CWR.toml"), &EmptyResourceResolver, &strict).is_err()
    );
}
