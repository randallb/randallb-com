use super::*;

fn ready(
    route: &mut ResolvedProcessAdapterRoute,
    invocation: &mut AdapterInvocation,
    store: &FileStore,
) -> CwrResult<Option<InputHandoff>> {
    handoff::prepare_with(route, invocation, &Default::default(), |id| {
        store.get(id)?.ok_or_else(|| "missing".into())
    })
}

#[test]
fn restored_queue_keeps_its_revision_and_reapproval_has_a_new_generation() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    let store = setup(root.path());
    let (route_a, invocation_a) = projected(root.path());
    let entry = environment::entries(root.path())
        .unwrap()
        .into_iter()
        .find(|e| e.kind == EnvironmentEntryKind::Variable)
        .unwrap();
    environment::save(
        root.path(),
        &store,
        "CONTEXT",
        EnvironmentEntryKind::Variable,
        VARIABLE,
        Some(&entry.reference),
        Some(entry.revision),
    )
    .unwrap();
    let (mut route_b, mut invocation_b) = projected(root.path());
    let serialized = serde_json::to_vec(&invocation_a).unwrap();
    let mut restored: AdapterInvocation = serde_json::from_slice(&serialized).unwrap();
    assert!(ready(&mut route_a.clone(), &mut restored, &store).is_err());
    drop(ready(&mut route_b, &mut invocation_b, &store).unwrap());
    let path = root.path().join("CWR.toml");
    let report = inspect_project_connections(&path).unwrap();
    let assignments = report
        .requirements
        .iter()
        .filter_map(|r| r.key_id.as_ref().map(|id| (r.alias.clone(), id.clone())))
        .collect();
    let revoked = save_project_connections(
        &path,
        &report.fingerprint,
        &report.grants_revision,
        BTreeMap::new(),
    )
    .unwrap();
    save_project_connections(
        &path,
        &revoked.fingerprint,
        &revoked.grants_revision,
        assignments,
    )
    .unwrap();
    let mut old: AdapterInvocation = serde_json::from_slice(&serialized).unwrap();
    assert!(ready(&mut route_a.clone(), &mut old, &store).is_err());
    let (mut route_c, mut invocation_c) = projected(root.path());
    assert_ne!(invocation_c.local_inputs, invocation_b.local_inputs);
    drop(ready(&mut route_c, &mut invocation_c, &store).unwrap());
}

#[test]
fn optional_absence_is_retained_and_new_work_uses_new_assignments() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    for file in ["CWR.toml", "shell.toml", "integration.toml"] {
        let path = root.path().join(file);
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace("[[inputs]]", "[[inputs]]\nrequired = false"),
        )
        .unwrap();
    }
    let graph = resolve_local_graph(&root.path().join("CWR.toml")).unwrap();
    let binding = &graph.actor_integrations[0];
    let project = || {
        binding.project_adapter_invocation(
            INVOCATION,
            "run--command",
            json!({}),
            json!({}),
            json!({}),
        )
    };
    let mut absent = project();
    assert_eq!(absent.local_inputs.as_ref().unwrap().dependencies.len(), 2);
    let store = FileStore(root.path().join("fake-keychain"));
    let mut route = binding.process_adapter_route().unwrap();
    drop(ready(&mut route, &mut absent, &store).unwrap());
    assert_eq!(absent.integration_params["env"], json!({}));
    let old = project();
    setup(root.path());
    let mut new = project();
    assert_ne!(old.local_inputs, new.local_inputs);
    assert!(ready(
        &mut binding.process_adapter_route().unwrap(),
        &mut old.clone(),
        &store
    )
    .is_err());
    drop(
        ready(
            &mut binding.process_adapter_route().unwrap(),
            &mut new,
            &store,
        )
        .unwrap(),
    );
    assert_eq!(new.integration_params["env"]["WORK_CONTEXT"], VARIABLE);
}

#[test]
fn workspace_execution_and_revocation_do_not_require_unrelated_stores() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    let store = setup(root.path());
    // Any attempt to open the legacy directory would fail.
    fs::remove_dir_all(root.path().join("legacy")).ok();
    fs::write(root.path().join("legacy"), "unavailable").unwrap();
    let (mut route, mut invocation) = projected(root.path());
    drop(ready(&mut route, &mut invocation, &store).unwrap());
    let path = root.path().join("CWR.toml");
    let report = inspect_project_connections(&path).unwrap();
    let database = root.path().join("state/WLP/wlpdb.sqlite3");
    fs::write(database, "not a database").unwrap();
    let revoked = save_project_connections(
        &path,
        &report.fingerprint,
        &report.grants_revision,
        BTreeMap::new(),
    )
    .unwrap();
    assert!(revoked.requirements.iter().all(|r| r.key_id.is_none()));
    assert!(revoked.environment_unavailable);
    assert!(read_grants(&path.canonicalize().unwrap())
        .unwrap()
        .connections
        .is_empty());
}

#[test]
fn legacy_only_execution_and_local_json_ignore_broken_wlp_database() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = super::super::tests::fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    let keys = crate::credentials::manage_api_keys(crate::credentials::CredentialRequest::Save {
        name: "test".into(),
        value: SECRET.into(),
        id: None,
    })
    .unwrap();
    let path = root.path().join("CWR.toml");
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap()
            + &format!("[[inputs]]\nas = 'profile'\ntype = '{PLAINTEXT_TYPE}'\n"),
    )
    .unwrap();
    fs::create_dir_all(root.path().join("state/WLP")).unwrap();
    fs::write(root.path().join("state/WLP/wlpdb.sqlite3"), "broken").unwrap();
    let report = inspect_project_connections(&path).unwrap();
    let saved = save_plaintext_connection(
        &path,
        &report.fingerprint,
        &report.grants_revision,
        "profile",
        json!({"name":"test"}),
    )
    .unwrap();
    let granted = save_project_connections(
        &path,
        &saved.fingerprint,
        &saved.grants_revision,
        BTreeMap::from([("research".into(), keys[0].id.clone())]),
    )
    .unwrap();
    assert!(granted.environment_unavailable);
    let graph = resolve_local_graph(&path).unwrap();
    let binding = &graph.actor_integrations[0];
    let mut invocation = binding.project_adapter_invocation(
        INVOCATION,
        "run--command",
        json!({}),
        json!({}),
        json!({}),
    );
    let mut route = binding.process_adapter_route().unwrap();
    drop(
        handoff::prepare_with(&mut route, &mut invocation, &Default::default(), |_| {
            panic!("legacy must not read Keychain")
        })
        .unwrap(),
    );
    assert_eq!(invocation.integration_params["env"]["RESEARCH_KEY"], SECRET);
}

#[test]
fn managed_destinations_and_untrusted_restored_templates_are_rejected() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    for destination in ["command", "runner", "endpoint", "prompt"] {
        let root = fixture();
        let _env = EnvironmentGuard::isolate(root.path());
        let path = root.path().join("integration.toml");
        let text = fs::read_to_string(&path).unwrap().replace(
            "[params.env]",
            &format!("[params]\n{destination} = {{ input = 'context' }}\n[params.env]"),
        );
        fs::write(path, text).unwrap();
        let report = inspect_project_connections(&root.path().join("CWR.toml")).unwrap();
        assert!(!report.warnings.is_empty(), "{destination}");
        let definition = definition(&root.path().join("CWR.toml")).unwrap();
        assert!(definition.graph.routes.is_empty(), "{destination}");
        assert!(
            definition.graph.actor_integrations.is_empty(),
            "{destination}"
        );
    }
    let root = fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    let store = setup(root.path());
    let (route, invocation) = projected(root.path());
    for mode in ["missing", "version", "destination", "binding", "parameters"] {
        let mut changed = invocation.clone();
        match mode {
            "missing" => changed.local_inputs = None,
            "version" => changed.local_inputs.as_mut().unwrap().version = 99,
            "destination" => changed.local_inputs.as_mut().unwrap().dependencies[0]
                .destinations
                .push(crate::input_delivery::InputDestination::ShellEnv {
                    name: "SMUGGLED".into(),
                }),
            "binding" => changed.local_inputs.as_mut().unwrap().binding_ref += "-other",
            _ => changed.integration_params["env"]["EXTRA"] = json!("different"),
        }
        assert!(
            ready(&mut route.clone(), &mut changed, &store).is_err(),
            "{mode}"
        );
    }
}

#[test]
fn configured_root_keeps_workspace_storage_and_bound_volumes() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    let workspace = root.path().join("WORKSPACE.toml");
    fs::write(&workspace, "type = 'cwp://org.coworkerprotocol/2026-09/schemas/workspace'\n[[volumes]]\nname = 'project'\npath = '.'\n[[runtimes]]\nname = 'main'\npath = './CWR.toml'\nsource_roots = ['.']\nworking_directory = '.'\nvolume = 'project'\n").unwrap();
    let store = setup(root.path());
    let entries = environment::entries(root.path()).unwrap();
    let assignments = entries
        .into_iter()
        .map(|e| {
            (
                (if e.kind == EnvironmentEntryKind::Secret {
                    "research"
                } else {
                    "context"
                })
                .into(),
                e.reference,
            )
        })
        .collect();
    let report = inspect_project_connections(&workspace).unwrap();
    assert!(project_connections_available(&workspace).unwrap());
    save_project_connections(
        &workspace,
        &report.fingerprint,
        &report.grants_revision,
        assignments,
    )
    .unwrap();
    let composition = LocalWorkspaceComposition::load(&workspace).unwrap();
    let graph = composition.runtimes[0].resolve_graph().unwrap();
    let binding = &graph.actor_integrations[0];
    let mut invocation = binding.project_adapter_invocation(
        INVOCATION,
        "run--command",
        json!({}),
        json!({}),
        json!({}),
    );
    drop(
        ready(
            &mut binding.process_adapter_route().unwrap(),
            &mut invocation,
            &store,
        )
        .unwrap(),
    );
    assert_eq!(
        invocation.integration_params["env"]["WORK_CONTEXT"],
        VARIABLE
    );
    // A multi-runtime workspace can still open and resolve its existing
    // configured routes, even though the single-runtime setup UI is unavailable.
    fs::write(&workspace, fs::read_to_string(&workspace).unwrap() + "[[runtimes]]\nname = 'second'\npath = './CWR.toml'\nsource_roots = ['.']\nworking_directory = '.'\nvolume = 'project'\n").unwrap();
    assert!(!project_connections_available(&workspace).unwrap());
    let composition = LocalWorkspaceComposition::load(&workspace).unwrap();
    assert_eq!(composition.runtimes.len(), 2);
    assert!(composition.runtimes[0].resolve_graph().is_ok());
}

#[test]
fn sibling_binding_changes_do_not_invalidate_an_unrelated_admission() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    let path = root.path().join("CWR.toml");
    fs::write(&path, fs::read_to_string(&path).unwrap() + &format!("[[inputs]]\nas = 'other_context'\ntype = '{PLAINTEXT_TYPE}'\nstorage = 'workspace_variable'\n[[imports]]\npath = './other-shell.toml'\nas = 'other'\n[[imports.inputs]]\nref = '~@/research'\nas = 'token'\n[[imports.inputs]]\nref = '~@/other_context'\nas = 'context'\n")).unwrap();
    fs::write(
        root.path().join("other-shell.toml"),
        fs::read_to_string(root.path().join("shell.toml"))
            .unwrap()
            .replace("slug = 'research'", "slug = 'other'")
            .replace("./integration.toml", "./other-integration.toml"),
    )
    .unwrap();
    fs::copy(
        root.path().join("integration.toml"),
        root.path().join("other-integration.toml"),
    )
    .unwrap();
    let store = setup(root.path());
    let entry = environment::save(
        root.path(),
        &store,
        "OTHER",
        EnvironmentEntryKind::Variable,
        "same",
        None,
        None,
    )
    .unwrap()
    .entries
    .into_iter()
    .find(|e| e.name == "OTHER")
    .unwrap();
    let report = inspect_project_connections(&path).unwrap();
    let mut assignments: BTreeMap<_, _> = report
        .requirements
        .iter()
        .filter_map(|r| r.key_id.as_ref().map(|id| (r.alias.clone(), id.clone())))
        .collect();
    assignments.insert("other_context".into(), entry.reference.clone());
    save_project_connections(
        &path,
        &report.fingerprint,
        &report.grants_revision,
        assignments,
    )
    .unwrap();
    let graph = resolve_local_graph(&path).unwrap();
    assert_eq!(graph.actor_integrations.len(), 2);
    let original = graph
        .actor_integrations
        .iter()
        .find(|b| b.input_uses.iter().any(|u| u.source.alias == "context"))
        .unwrap();
    let sibling = graph
        .actor_integrations
        .iter()
        .find(|b| {
            b.input_uses
                .iter()
                .any(|u| u.source.alias == "other_context")
        })
        .unwrap();
    let mut before = original.project_adapter_invocation(
        INVOCATION,
        "run--command",
        json!({}),
        json!({}),
        json!({}),
    );
    let mut other_before = sibling.project_adapter_invocation(
        INVOCATION,
        "run--command",
        json!({}),
        json!({}),
        json!({}),
    );
    environment::save(
        root.path(),
        &store,
        "OTHER",
        EnvironmentEntryKind::Variable,
        "same",
        Some(&entry.reference),
        Some(entry.revision),
    )
    .unwrap();
    drop(
        ready(
            &mut original.process_adapter_route().unwrap(),
            &mut before,
            &store,
        )
        .unwrap(),
    );
    assert!(ready(
        &mut sibling.process_adapter_route().unwrap(),
        &mut other_before,
        &store
    )
    .is_err());
}

#[test]
fn local_json_object_is_inspectable_before_its_first_save() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = super::super::tests::fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    for file in ["CWR.toml", "shell.toml", "integration.toml"] {
        let path = root.path().join(file);
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace(SECRET_TYPE, PLAINTEXT_TYPE);
        fs::write(
            &path,
            if file == "integration.toml" {
                text.replace(
                    "[params.env]\nRESEARCH_KEY = { input = 'token' }",
                    "[params]\nenv = { input = 'token' }",
                )
            } else {
                text
            },
        )
        .unwrap();
    }
    let path = root.path().join("CWR.toml");
    let report = inspect_project_connections(&path).unwrap();
    assert!(!report.requirements[0].configured);
    let saved = save_plaintext_connection(
        &path,
        &report.fingerprint,
        &report.grants_revision,
        "research",
        json!({"MODE":"test"}),
    )
    .unwrap();
    assert!(saved.requirements[0].configured);
    assert_eq!(
        resolve_local_graph(&path).unwrap().actor_integrations[0].integration_params["env"]["MODE"],
        "test"
    );
}
