#[path = "connections/imported_volume_tests.rs"]
mod imported_volume_tests;

use super::*;

fn write_imported_bindings(path: &Path, bindings: serde_json::Value) {
    let file = super::super::imported_volumes::bindings_path(path).unwrap();
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, serde_json::to_vec(&bindings).unwrap()).unwrap();
}

pub(super) fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("CWR.toml"), format!(
        "type = '{}'\n[[inputs]]\nas = 'research'\ntype = '{SECRET_TYPE}'\npurpose = 'Research'\n[[imports]]\npath = './person.toml'\n", graph::RUNTIME_TYPE)).unwrap();
    fs::write(root.path().join("person.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Person'\nslug = 'person'\n").unwrap();
    let path = root.path().join("CWR.toml");
    fs::write(&path, fs::read_to_string(&path).unwrap() + "[[imports]]\npath = './shell.toml'\n[[imports.inputs]]\nref = '~@/research'\nas = 'token'\n").unwrap();
    fs::write(root.path().join("shell.toml"), format!("type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Research'\nslug = 'research'\n[[inputs]]\nas = 'token'\ntype = '{SECRET_TYPE}'\n[[roles]]\npath = './role.toml'\n[[integrations]]\npath = './integration.toml'\nas = 'shell'\n[[integrations.inputs]]\nref = '~@/token'\nas = 'token'\n")).unwrap();
    fs::write(root.path().join("role.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n").unwrap();
    fs::write(root.path().join("integration.toml"), format!("type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[[inputs]]\nas = 'token'\ntype = '{SECRET_TYPE}'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n[params.env]\nRESEARCH_KEY = {{ input = 'token' }}\n")).unwrap();
    root
}

#[test]
fn ordinary_workspace_loads_imported_volume_bindings_from_local_state() {
    let _lock = crate::workspace_environment::TEST_ENVIRONMENT_LOCK
        .lock()
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let _environment = super::handoff_tests::EnvironmentGuard::isolate(root.path());
    let path = root.path().join("CWR.toml");
    let loop_path = root.path().join("loop/CWR.toml");
    let memory = root.path().join("memory");
    fs::create_dir_all(loop_path.parent().unwrap()).unwrap();
    fs::create_dir(&memory).unwrap();
    fs::write(
        &path,
        format!(
            "type = '{}'\n[[imports]]\npath = './loop/CWR.toml'\nas = 'loop'\n",
            graph::ROOT_TYPE
        ),
    )
    .unwrap();
    fs::write(&loop_path, format!("type = '{}'\n[[volumes]]\nas = 'memory'\naccess = 'read_write'\n[[imports]]\npath = './person.toml'\nas = 'person'\n", graph::RUNTIME_TYPE)).unwrap();
    fs::write(root.path().join("loop/person.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Person'\nslug = 'person'\n").unwrap();
    let unavailable = resolve_local_graph(&path).unwrap();
    assert!(unavailable.actors.is_empty());
    assert!(!unavailable.warnings.is_empty());
    write_imported_bindings(
        &path,
        json!({
            "version": 1,
            "bindings": [{"source": loop_path, "alias": "memory", "path": memory, "access": "read_write"}]
        }),
    );
    let graph = resolve_local_graph(&path).unwrap();
    assert_eq!(graph.actors.len(), 1);
    assert_eq!(
        crate::v2026_09::host::inspect_local_workspace(&path)
            .unwrap()
            .actors
            .len(),
        1
    );
    let identity =
        LocalHostIdentity::new(graph.actors[0].actor_ref.clone(), "cwp://example.test/cli");
    let state = root.path().join("state.sqlite3");
    let runtime = LocalRuntime::open_with_state_path(&path, &state, identity.clone()).unwrap();
    let actor_ref = runtime.catalog().unwrap().acting_actor.actor_ref;
    let admission = runtime
        .submit_text_as(
            "cwp://example.test/cli",
            "memory-conversation",
            &actor_ref,
            None,
            "Remember the school deadline",
        )
        .unwrap();
    let conversation_ref = admission.conversation_ref().to_owned();
    let replacement = root.path().join("replacement");
    fs::create_dir(&replacement).unwrap();
    write_imported_bindings(
        &path,
        json!({
            "version": 1,
            "bindings": [{"source": loop_path, "alias": "memory", "path": replacement, "access": "read_write"}]
        }),
    );
    assert!(runtime
        .reload_graph()
        .unwrap_err()
        .to_string()
        .contains("reopen"));
    drop(runtime);
    let reopened = LocalRuntime::open_with_state_path(&path, &state, identity).unwrap();
    assert_eq!(
        reopened.catalog().unwrap().acting_actor.actor_ref,
        actor_ref
    );
    let conversation = reopened.conversation(&conversation_ref).unwrap().unwrap();
    assert!(conversation
        .messages
        .iter()
        .any(|message| message.text == "Remember the school deadline"));
    drop(reopened);
    fs::remove_dir(root.path().join("memory")).unwrap();
    fs::remove_dir(root.path().join("replacement")).unwrap();
    assert!(resolve_local_graph(&path).is_err());
}

#[test]
fn live_reload_rejects_new_imported_volume_grants() {
    let _lock = crate::workspace_environment::TEST_ENVIRONMENT_LOCK
        .lock()
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let _environment = super::handoff_tests::EnvironmentGuard::isolate(root.path());
    let path = root.path().join("CWR.toml");
    let loop_path = root.path().join("loop/CWR.toml");
    let memory = root.path().join("memory");
    fs::create_dir(root.path().join("loop")).unwrap();
    fs::create_dir(&memory).unwrap();
    fs::write(
        &path,
        format!(
            "type = '{}'\n[[imports]]\npath = './loop/CWR.toml'\nas = 'loop'\n",
            graph::ROOT_TYPE
        ),
    )
    .unwrap();
    fs::write(
        &loop_path,
        format!(
            "type = '{}'\n[[imports]]\npath = './person.toml'\nas = 'person'\n",
            graph::RUNTIME_TYPE
        ),
    )
    .unwrap();
    fs::write(root.path().join("loop/person.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Person'\nslug = 'person'\n").unwrap();
    let graph = resolve_local_graph(&path).unwrap();
    let identity =
        LocalHostIdentity::new(graph.actors[0].actor_ref.clone(), "cwp://example.test/cli");
    let runtime =
        LocalRuntime::open_with_state_path(&path, root.path().join("state.sqlite3"), identity)
            .unwrap();
    fs::write(
        &loop_path,
        fs::read_to_string(&loop_path).unwrap()
            + "[[volumes]]\nas = 'memory'\naccess = 'read_write'\n",
    )
    .unwrap();
    write_imported_bindings(
        &path,
        json!({
            "version": 1,
            "bindings": [{"source": loop_path, "alias": "memory", "path": memory, "access": "read_write"}]
        }),
    );
    assert!(runtime
        .reload_graph()
        .unwrap_err()
        .to_string()
        .contains("reopen"));
}

#[test]
fn definition_inspects_without_credentials_and_changes_need_new_approval() {
    let root = fixture();
    let path = root.path().join("CWR.toml");
    let first = definition(&path).unwrap();
    assert_eq!(first.requirements.len(), 1);
    assert_eq!(first.requirements[0].alias, "research");
    assert_eq!(first.fingerprint, definition(&path).unwrap().fingerprint);
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("Research", "New purpose"),
    )
    .unwrap();
    assert_ne!(first.fingerprint, definition(&path).unwrap().fingerprint);
    assert!(!root.path().join(".cwr").exists());
}

#[test]
fn unassigned_inputs_do_not_need_the_local_credential_store() {
    let root = fixture();
    let target = connection_target(&root.path().join("CWR.toml")).unwrap();
    let definition = definition_for_target(&target).unwrap();
    let resolution = resolve(&target, &definition, &Grants::default(), None).unwrap();
    assert!(!resolution.rows[0].approved);
}

#[test]
fn plaintext_inputs_are_reported_as_local_configuration_not_keys() {
    let _lock = crate::workspace_environment::TEST_ENVIRONMENT_LOCK
        .lock()
        .unwrap();
    let root = fixture();
    let _environment = super::handoff_tests::EnvironmentGuard::isolate(root.path());
    let path = root.path().join("CWR.toml");
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap()
            + &format!(
                "[[inputs]]\nas = 'family_profile'\ntype = '{PLAINTEXT_TYPE}'\npurpose = 'Local family profile'\n"
            ),
    )
    .unwrap();
    let report = inspect_project_connections(&path).unwrap();
    let profile = report
        .requirements
        .iter()
        .find(|requirement| requirement.alias == "family_profile")
        .unwrap();
    assert_eq!(profile.type_ref, PLAINTEXT_TYPE);
    assert!(profile.key_id.is_none());
    assert!(!profile.approved);
}

#[test]
fn plaintext_configuration_stays_local_and_reopens_without_credentials() {
    let _lock = crate::workspace_environment::TEST_ENVIRONMENT_LOCK
        .lock()
        .unwrap();
    let root = fixture();
    let _environment = super::handoff_tests::EnvironmentGuard::isolate(root.path());
    let path = root.path().join("CWR.toml");
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap()
            + &format!(
                "[[inputs]]\nas = 'family_profile'\ntype = '{PLAINTEXT_TYPE}'\npurpose = 'Local family profile'\n"
            ),
    )
    .unwrap();
    let report = inspect_project_connections(&path).unwrap();
    let saved = save_plaintext_connection(
        &path,
        &report.fingerprint,
        &report.grants_revision,
        "family_profile",
        json!({"family_name": "Whitney"}),
    )
    .unwrap();
    assert!(
        saved
            .requirements
            .iter()
            .find(|requirement| requirement.alias == "family_profile")
            .unwrap()
            .configured
    );
    assert_eq!(
        graph_options(&path).unwrap().inputs["family_profile"],
        json!({"family_name": "Whitney"})
    );
    let graph = resolve_local_graph(&path).unwrap();
    assert!(!serde_json::to_string(&graph)
        .unwrap()
        .contains("disposable-connection-secret"));
    let metadata_path = grants_path(&path.canonicalize().unwrap()).unwrap();
    let metadata = fs::read_to_string(&metadata_path).unwrap();
    assert!(!metadata.contains(root.path().to_string_lossy().as_ref()));
    assert!(!metadata.lines().any(|line| line.starts_with("revision =")));
    fs::remove_file(metadata_path).unwrap();
}

#[test]
fn stale_grants_revision_cannot_replace_a_newer_local_configuration() {
    let _lock = crate::workspace_environment::TEST_ENVIRONMENT_LOCK
        .lock()
        .unwrap();
    let root = fixture();
    let _environment = super::handoff_tests::EnvironmentGuard::isolate(root.path());
    let path = root.path().join("CWR.toml");
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap()
            + &format!(
                "[[inputs]]\nas = 'family_profile'\ntype = '{PLAINTEXT_TYPE}'\npurpose = 'Local family profile'\n"
            ),
    )
    .unwrap();
    let first = inspect_project_connections(&path).unwrap();
    let stale = inspect_project_connections(&path).unwrap();
    assert_eq!(first.grants_revision, stale.grants_revision);

    let saved = save_plaintext_connection(
        &path,
        &first.fingerprint,
        &first.grants_revision,
        "family_profile",
        json!({"family_name": "Whitney"}),
    )
    .unwrap();
    assert_ne!(saved.grants_revision, first.grants_revision);

    let error = save_plaintext_connection(
        &path,
        &stale.fingerprint,
        &stale.grants_revision,
        "family_profile",
        json!({"family_name": "Stale"}),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("connections changed"));

    let current = inspect_project_connections(&path).unwrap();
    assert_eq!(current.grants_revision, saved.grants_revision);
    assert_eq!(
        current
            .requirements
            .iter()
            .find(|requirement| requirement.alias == "family_profile")
            .and_then(|requirement| requirement.local_value.as_ref()),
        Some(&json!({"family_name": "Whitney"}))
    );
    fs::remove_file(grants_path(&path.canonicalize().unwrap()).unwrap()).unwrap();
}

#[test]
fn materialization_changes_only_the_declared_slot() {
    let mut route = crate::adapter::ResolvedProcessAdapterRoute::new(
        crate::adapter::SH_ADAPTER_REF,
        "/tmp",
        "/tmp",
        json!({}),
        json!({"env": {"KEY": "same", "LITERAL": "same"}, "command": "same"}),
        json!({}),
    );
    route
        .materialize_input(
            &crate::input_delivery::InputDestination::ShellEnv { name: "KEY".into() },
            "dummy-value",
        )
        .unwrap();
    let value = serde_json::to_value(route).unwrap();
    assert_eq!(value["integration_params"]["env"]["KEY"], "dummy-value");
    assert_eq!(value["integration_params"]["env"]["LITERAL"], "same");
    assert_eq!(value["integration_params"]["command"], "same");
}

#[test]
fn keys_cannot_be_substituted_into_runner_arguments() {
    let root = fixture();
    let integration = root.path().join("integration.toml");
    let text = fs::read_to_string(&integration).unwrap();
    fs::write(
        integration,
        text.replace(
            "[params.env]\nRESEARCH_KEY = { input = 'token' }",
            "[params]\nrunner = ['/bin/sh', { input = 'token' }]",
        ),
    )
    .unwrap();
    let definition = definition(&root.path().join("CWR.toml")).unwrap();
    assert!(definition.graph.actor_integrations.is_empty());
    assert!(definition.graph.routes.is_empty());
    assert!(!definition.graph.warnings.is_empty());
}

#[test]
fn legacy_grants_round_trip_and_revoke_without_retaining_values() {
    // This dispatch needs the real adapter while other tests replace its executable.
    let _adapter_lock = crate::v2026_09::local::tests::SH_ADAPTER_ENV_LOCK
        .lock()
        .unwrap();
    use crate::credentials::{manage_api_keys, CredentialRequest};
    let _lock = crate::workspace_environment::TEST_ENVIRONMENT_LOCK
        .lock()
        .unwrap();
    let root = fixture();
    let _environment = super::handoff_tests::EnvironmentGuard::isolate(root.path());
    let path = root.path().join("CWR.toml");
    let key_name = format!("Connections test {}", Uuid::new_v4());
    let keys = manage_api_keys(CredentialRequest::Save {
        name: key_name.clone(),
        value: "disposable-connection-secret".into(),
        id: None,
    })
    .unwrap();
    let id = keys.into_iter().find(|k| k.name == key_name).unwrap().id;
    let result = (|| -> CwrResult<()> {
        let report = inspect_project_connections(&path)?;
        assert!(!report.requirements[0].approved);
        let report = save_project_connections(
            &path,
            &report.fingerprint,
            &report.grants_revision,
            BTreeMap::from([("research".into(), id.clone())]),
        )?;
        assert!(report.requirements[0].approved);
        assert!(resolve_local_graph(&path).is_ok());
        let graph = resolve_local_graph(&path)?;
        let mut route = graph.actor_integrations[0].process_adapter_route()?;
        let mut invocation = graph.actor_integrations[0].project_adapter_invocation(
            "test",
            "run--command",
            json!({}),
            json!({}),
            json!({}),
        );
        invocation.integration_ref = Some(graph.actor_integrations[0].integration_ref.clone());
        let retained = serde_json::to_string(&invocation)?;
        let mut altered: Value = serde_json::to_value(&route)?;
        altered["integration_params"]["runner"] = json!(["/bin/bash", "-c", "changed-destination"]);
        let mut altered_route = serde_json::from_value(altered)?;
        assert!(prepare_handoff(
            &mut altered_route,
            &mut invocation.clone(),
            &Default::default()
        )
        .is_err());
        assert!(!retained.contains("disposable-connection-secret"));
        let guard = prepare_handoff(&mut route, &mut invocation, &Default::default())?;
        drop(guard);
        assert_eq!(
            invocation.integration_params["env"]["RESEARCH_KEY"],
            "disposable-connection-secret"
        );
        let metadata = fs::read_to_string(grants_path(&path.canonicalize()?)?)?;
        assert!(!metadata.contains("disposable-connection-secret"));
        assert!(!root.path().join(".cwr").exists());
        // Exercise the same local dispatch and SQLite retention used by Desktop.
        let actor = load_default_local_acting_actor_ref(&path)?;
        let runtime = LocalRuntime::open_with_state_path(
            &path,
            root.path().join("cwpdb.sqlite3"),
            LocalHostIdentity::new(&actor, "cwp://example.test/host"),
        )?;
        let admission = runtime.submit_text_as(
            "cwp://example.test/host",
            "check",
            &actor,
            None,
            "@research test -n \"$RESEARCH_KEY\" && printf connected",
        )?;
        let mut completed = false;
        runtime.execute_admission(&admission, |event| {
            let wire = serde_json::to_string(&event).unwrap();
            assert!(!wire.contains("disposable-connection-secret"));
            if wire.contains("connected") {
                completed = true;
            }
        })?;
        assert!(
            completed,
            "the shell integration did not receive its assigned key"
        );
        drop(runtime);
        for entry in fs::read_dir(root.path())? {
            let entry = entry?;
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("cwpdb.sqlite3")
            {
                assert!(!String::from_utf8_lossy(&fs::read(entry.path())?)
                    .contains("disposable-connection-secret"));
            }
        }
        fs::write(
            &path,
            fs::read_to_string(&path)?.replace("Research", "Changed request"),
        )?;
        assert!(!inspect_project_connections(&path)?.requirements[0].approved);
        assert!(save_project_connections(
            &path,
            &report.fingerprint,
            &report.grants_revision,
            BTreeMap::new()
        )
        .is_err());
        let mut old: AdapterInvocation = serde_json::from_str(&retained)?;
        let mut old_route = old.retained_route.clone().unwrap();
        assert!(prepare_handoff(&mut old_route, &mut old, &Default::default()).is_err());
        assert!(!inspect_project_connections(&path)?.requirements[0].approved);
        let updated = inspect_project_connections(&path)?;
        let updated = save_project_connections(
            &path,
            &updated.fingerprint,
            &updated.grants_revision,
            BTreeMap::from([("research".into(), id.clone())]),
        )?;
        manage_api_keys(CredentialRequest::Remove { id: id.clone() })?;
        assert!(!inspect_project_connections(&path)?.requirements[0].approved);
        // Revocation removes retrieval authority, independently of saved key existence.
        save_project_connections(
            &path,
            &updated.fingerprint,
            &updated.grants_revision,
            BTreeMap::new(),
        )?;
        assert!(!inspect_project_connections(&path)?.requirements[0].approved);
        Ok(())
    })();
    let _ = fs::remove_file(grants_path(&path.canonicalize().unwrap()).unwrap());
    manage_api_keys(CredentialRequest::Remove { id }).unwrap();
    result.unwrap();
}

#[test]
fn connection_fingerprint_preserves_historical_graph_encoding() {
    let root = fixture();
    let path = root.path().join("CWR.toml");
    let source = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        source.replace(
            graph::RUNTIME_TYPE,
            "cwp://org.coworkerprotocol/2026-09/schemas/loop",
        ),
    )
    .unwrap();
    let current = definition(&path).unwrap();
    let mut historical = serde_json::to_value(&current.graph).unwrap();
    let fields = historical.as_object_mut().unwrap();
    let reference = fields.remove("runtime_ref").unwrap();
    fields.insert("configured_loop_ref".into(), reference);
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&historical).unwrap());
    let mut sources: BTreeSet<_> = current
        .graph
        .resources
        .iter()
        .map(|r| r.source.clone())
        .collect();
    sources.insert(current.path.clone());
    for source in sources {
        digest.update(fs::read(source).unwrap());
    }
    assert_eq!(current.fingerprint, hex::encode(digest.finalize()));
    assert!(serde_json::to_value(&current.graph)
        .unwrap()
        .get("runtime_ref")
        .is_some());
}
