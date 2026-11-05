use super::*;

#[path = "runtime_projection_tests.rs"]
mod runtime_projection_tests;

#[path = "installed_pack_grants_tests.rs"]
mod installed_pack_grants_tests;

#[path = "configured_identity_integration_tests.rs"]
mod configured_identity_integration_tests;

fn workspace(temporary: &Path, runtimes: &str) -> PathBuf {
    fs::create_dir_all(temporary.join("source")).unwrap();
    fs::create_dir_all(temporary.join("volume")).unwrap();
    fs::write(temporary.join("source/COWORKER.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Root'\nslug = 'root'\n").unwrap();
    fs::write(temporary.join("source/CWR.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/loop'\n[[imports]]\npath = './COWORKER.toml'\n").unwrap();
    let path = temporary.join("WORKSPACE.toml");
    fs::write(
        &path,
        format!("type = 'cwp://org.coworkerprotocol/2026-09/schemas/workspace'\n[[volumes]]\nname = 'shared'\npath = './volume'\n{runtimes}"),
    )
    .unwrap();
    path
}

fn declaration(name: &str) -> String {
    format!(
        "[[runtimes]]\nname = '{name}'\npath = './source/CWR.toml'\nsource_roots = ['./source']\nworking_directory = '.'\nvolume = 'shared'\n"
    )
}

#[test]
fn shared_sources_and_volumes_keep_separate_runtime_context_and_state() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(
        temporary.path(),
        &(declaration("one") + &declaration("two")),
    );
    let first = LocalWorkspaceComposition::load(&path).unwrap();
    let second = LocalWorkspaceComposition::load(&path).unwrap();
    let one = &first.runtimes[0];
    let two = &first.runtimes[1];
    assert_eq!(one.cwr_toml, two.cwr_toml);
    assert_ne!(one.runtime_ref, two.runtime_ref);
    assert_ne!(one.database_path(), two.database_path());
    assert_eq!(one.runtime_ref, second.runtimes[0].runtime_ref);
    assert_eq!(
        one.working_directory,
        temporary.path().canonicalize().unwrap()
    );
    assert!(
        !one.state_directory.exists(),
        "inspection must not create state"
    );
    first.check().unwrap();
    let one_graph = one.resolve_graph().unwrap();
    let two_graph = two.resolve_graph().unwrap();
    assert_eq!(one_graph.reference, two_graph.reference);
    assert_ne!(one_graph.runtime_ref, two_graph.runtime_ref);
}

#[test]
fn configured_runtime_rejects_independent_rollout_flags() {
    for field in ["conversation_timeline_rollout = 'new_conversations'", "tool_activity_capture = false"] {
        let temporary=tempfile::tempdir().unwrap();
        let path=workspace(temporary.path(), &(declaration("one") + field + "\n"));
        assert!(LocalWorkspaceComposition::load(path).is_err());
    }
}

#[test]
fn resident_fingerprint_changes_with_execution_bindings() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    fs::write(
        temporary.path().join("source/CWR.toml"),
        fs::read_to_string(temporary.path().join("source/CWR.toml")).unwrap()
            + "[[volumes]]\nas = 'project'\naccess = 'read'\n",
    )
    .unwrap();
    fs::create_dir(temporary.path().join("alternate-source")).unwrap();
    let mut composition = LocalWorkspaceComposition::load(path).unwrap();
    let configured = &mut composition.runtimes[0];
    configured.workspace_volumes.push(LocalWorkspaceVolume {
        name: "alternate".into(),
        path: temporary.path().join("volume"),
        access: LocalVolumeAccessMode::Read,
        required: true,
        purpose: String::new(),
    });
    configured.volume_mappings.push(VolumeMapping {
        reference: "~@/shared".into(),
        alias: "project".into(),
        access: LocalVolumeAccessMode::Read,
    });
    let original = configured.prepare_resident().unwrap().fingerprint;

    configured.volume_mappings[0].reference = "~@/alternate".into();
    let remapped = configured.prepare_resident().unwrap().fingerprint;
    assert_ne!(original, remapped);

    configured
        .source_roots
        .push(temporary.path().join("alternate-source"));
    let new_source_root = configured.prepare_resident().unwrap().fingerprint;
    assert_ne!(remapped, new_source_root);

    configured.volume = "alternate".into();
    let new_primary = configured.prepare_resident().unwrap().fingerprint;
    assert_ne!(new_source_root, new_primary);
}

#[test]
fn rejects_duplicate_runtimes_and_unknown_volumes() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(
        temporary.path(),
        &(declaration("one") + &declaration("one")),
    );
    assert!(
        LocalWorkspaceComposition::load(&path)
            .unwrap_err()
            .to_string()
            .contains("duplicate configured runtime")
    );
    let path = workspace(
        temporary.path(),
        &declaration("one").replace("volume = 'shared'", "volume = 'missing'"),
    );
    assert!(
        LocalWorkspaceComposition::load(&path)
            .unwrap_err()
            .to_string()
            .contains("unknown volume")
    );
}

#[test]
fn source_access_does_not_follow_symlinks_outside_assigned_roots() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    fs::create_dir(temporary.path().join("outside")).unwrap();
    fs::write(
        temporary.path().join("outside/CWR.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas'\n",
    )
    .unwrap();
    fs::remove_file(temporary.path().join("source/CWR.toml")).unwrap();
    std::os::unix::fs::symlink(
        temporary.path().join("outside/CWR.toml"),
        temporary.path().join("source/CWR.toml"),
    )
    .unwrap();
    assert!(
        LocalWorkspaceComposition::load(path)
            .unwrap_err()
            .to_string()
            .contains("outside its source roots")
    );
}

#[test]
fn environment_binding_errors_do_not_include_secret_values() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    let mut composition = LocalWorkspaceComposition::load(path).unwrap();
    composition.runtimes[0].inputs =
        json!({"credential": {"env": "CWR_TEST_MISSING_934980449_INPUT"}});
    let error = composition.runtimes[0].resolve_graph().unwrap_err();
    assert!(error.to_string().contains("credential"));
    assert!(
        !error
            .to_string()
            .contains("CWR_TEST_MISSING_934980449_INPUT")
    );
}

#[test]
fn catalog_inspection_neither_creates_nor_locks_runtime_state() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    let mut composition = LocalWorkspaceComposition::load(&path).unwrap();
    composition.runtimes[0].state_directory = temporary.path().join("state/CWR/loops/one");
    let configured = &composition.runtimes[0];
    composition.check().unwrap();
    let inspected = configured.catalog().unwrap();
    assert!(
        inspected
            .known_actors
            .iter()
            .any(|actor| actor.handle == "@root")
    );
    assert!(
        inspected
            .known_actors
            .iter()
            .any(|actor| actor.handle == "@cwr")
    );
    assert!(!configured.state_directory.exists());

    let runtime = configured.open("cwp://example.test/cli").unwrap();
    let while_open = configured.catalog().unwrap();
    let mut live = runtime.catalog().unwrap();
    // Inspection resolves the graph without reading or locking persisted history.
    assert!(while_open.retained_actors.is_empty());
    assert_eq!(live.retained_actors.len(), live.known_actors.len());
    live.retained_actors.clear();
    assert_eq!(
        serde_json::to_value(while_open).unwrap(),
        serde_json::to_value(live).unwrap(),
    );
}

#[test]
fn workspace_check_rejects_reserved_coworkers_before_opening_state() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    let source = temporary.path().join("source/COWORKER.toml");
    fs::write(
        &source,
        fs::read_to_string(&source)
            .unwrap()
            .replace("slug = 'root'", "slug = 'cwr'"),
    )
    .unwrap();
    let composition = LocalWorkspaceComposition::load(&path).unwrap();
    assert!(
        composition
            .check()
            .unwrap_err()
            .to_string()
            .contains("reserved @cwr")
    );
    assert!(!composition.runtimes[0].state_directory.exists());
}

#[test]
fn workspace_check_rejects_ambiguous_native_tools_before_opening_state() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    let source = temporary.path().join("source");
    let root = source.join("COWORKER.toml");
    let native = "[[native_tools]]\ncoworker = '~@/helper'\naction = 'run--command'\n";
    fs::write(
        &root,
        fs::read_to_string(&root).unwrap()
            + "[[imports]]\npath = './HELPER.toml'\nas = 'helper'\n"
            + native
            + native,
    )
    .unwrap();
    fs::write(source.join("HELPER.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Helper'\n[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = './SH.toml'\nas = 'sh'\n").unwrap();
    fs::write(source.join("SH.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n").unwrap();
    fs::write(source.join("ROLE.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n[[actions]]\nverb = 'run'\nobject = 'command'\ntitle = 'Run'\ndescription = 'Run a command'\n[actions.input_schema]\npath = './schema.json'\n[actions.output_schema]\npath = './schema.json'\n").unwrap();
    fs::write(source.join("schema.json"), r#"{"type":"object"}"#).unwrap();
    let composition = LocalWorkspaceComposition::load(&path).unwrap();
    assert!(
        composition
            .check()
            .unwrap_err()
            .to_string()
            .contains("ambiguous native tool name")
    );
    assert!(!composition.runtimes[0].state_directory.exists());
}

#[test]
fn inspection_rejects_intrinsic_cwr_as_the_configured_acting_coworker() {
    let temporary = tempfile::tempdir().unwrap();
    for actor in ["cwr", "@cwr", INTRINSIC_CWR_ACTOR_REF] {
        let path = workspace(
            temporary.path(),
            &(declaration("one") + &format!("acting_coworker = '{actor}'\n")),
        );
        let composition = LocalWorkspaceComposition::load(path).unwrap();
        let checked = composition.check().unwrap_err().to_string();
        let opened = composition.runtimes[0]
            .open("cwp://example.test/cli")
            .err()
            .expect("an intrinsic actor cannot act for the configured runtime")
            .to_string();
        assert!(checked.contains("acting_coworker must select one loaded coworker"));
        assert_eq!(checked, opened);
        assert!(!composition.runtimes[0].state_directory.exists());
    }
}

#[test]
fn composition_accepts_named_loop_definitions() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    fs::rename(
        temporary.path().join("source/CWR.toml"),
        temporary.path().join("source/ROOT.toml"),
    )
    .unwrap();
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("source/CWR.toml", "source/ROOT.toml"),
    )
    .unwrap();
    let mut composition = LocalWorkspaceComposition::load(path).unwrap();
    composition.runtimes[0].state_directory = temporary.path().join("state/CWR/loops/one");
    composition.check().unwrap();
    composition.runtimes[0]
        .open("cwp://example.test/cli")
        .unwrap();
    assert!(!temporary.path().join("volume/.cwr").exists());
}

#[test]
fn overridden_state_directory_preserves_ancestor_permissions() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    let mut composition = LocalWorkspaceComposition::load(path).unwrap();
    let ancestor = temporary.path().join("shared");
    let parent = ancestor.join("project");
    fs::create_dir_all(&parent).unwrap();
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o775)).unwrap();
    let configured = &mut composition.runtimes[0];
    configured.state_directory = parent.join("loop");
    // Both initial creation and reopening must leave shared ancestors alone.
    for _ in 0..2 {
        let runtime = configured.open("cwp://example.test/cli").unwrap();
        assert_eq!(
            fs::metadata(&ancestor).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
            0o775
        );
        assert_eq!(
            fs::metadata(&configured.state_directory)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        drop(runtime);
        fs::set_permissions(
            &configured.state_directory,
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
}

#[test]
fn private_state_supports_read_only_volumes_and_preserves_history_on_reopen() {
    let temporary = tempfile::tempdir().unwrap();
    let loops = declaration("one")
        + "[[runtimes.volume_access]]\nvolume = 'shared'\naccess = 'read'\n"
        + &declaration("two");
    let path = workspace(temporary.path(), &loops);
    let mut composition = LocalWorkspaceComposition::load(path).unwrap();
    for configured in &mut composition.runtimes {
        assert!(
            configured
                .state_directory
                .starts_with(platform_state_home().unwrap())
        );
        assert!(
            !configured
                .state_directory
                .starts_with(&composition.volumes[0].path)
        );
        configured.state_directory = temporary
            .path()
            .join("state/CWR/loops")
            .join(&configured.name);
    }
    let volume = &composition.volumes[0].path;
    fs::set_permissions(volume, fs::Permissions::from_mode(0o555)).unwrap();
    let one = &composition.runtimes[0];
    let runtime = one.open("cwp://example.test/cli").unwrap();
    let admission = runtime
        .admit_action(None, "@cwr--list--coworkers", json!({}))
        .unwrap();
    let conversation = admission.conversation_ref();
    let two = composition.runtimes[1]
        .open("cwp://example.test/cli")
        .unwrap();
    assert!(two.conversation(conversation).unwrap().is_none());
    assert!(
        one.open("cwp://example.test/cli").is_err(),
        "one executor owns the database"
    );
    composition.check().unwrap();
    drop(runtime);
    let reopened = one.open("cwp://example.test/cli").unwrap();
    assert!(reopened.conversation(conversation).unwrap().is_some());
    assert_eq!(
        fs::metadata(&one.state_directory)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(!volume.join(".cwr").exists());
    fs::set_permissions(volume, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn composition_validates_types_and_fields_before_resolving_inputs() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    let valid = fs::read_to_string(&path).unwrap();
    for (source, expected) in [
        (
            valid.replace(
                "type = 'cwp://org.coworkerprotocol/2026-09/schemas/workspace'",
                "version = 1",
            ),
            "version is retired",
        ),
        (
            valid.replacen("type =", "unsupported =", 1),
            "missing or unsupported",
        ),
        (
            valid.replace("schemas/workspace", "schemas/unknown"),
            "missing or unsupported",
        ),
        (
            valid.clone() + "unexpected = 'never disclose this value'\n",
            "invalid WORKSPACE.toml fields",
        ),
    ] {
        fs::write(&path, source).unwrap();
        let error = LocalWorkspaceComposition::load(&path)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("never disclose"));
    }
    fs::write(
        &path,
        valid + "[runtimes.inputs]\nsecret = { env = 'CWR_TEST_MISSING_934980449_INPUT' }\n",
    )
    .unwrap();
    fs::write(temporary.path().join("source/CWR.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Person'\nslug = 'person'\n").unwrap();
    let error = LocalWorkspaceComposition::load(&path)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("must select a reusable runtime definition"),
        "{error}"
    );
}

#[test]
fn graph_imports_loop_definitions_without_opening_configured_contexts() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    let graph_path = temporary.path().join("source/graph.toml");
    fs::write(&graph_path, "type = 'cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath = './CWR.toml'\nas = 'reusable'\n").unwrap();
    let checked = crate::v2026_09::graph::check(&graph_path).unwrap();
    assert!(
        checked["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|resource| {
                resource["type"] == "cwp://org.coworkerprotocol/2026-09/schemas/loop"
            })
    );
    let composition = LocalWorkspaceComposition::load(path).unwrap();
    assert!(!composition.runtimes[0].state_directory.exists());
    assert!(!temporary.path().join("volume/.cwr").exists());
}

#[test]
fn prepared_resident_checks_prompt_contents_and_enforces_actor_before_state() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(
        temporary.path(),
        &(declaration("one") + "acting_coworker = '@root'\n"),
    );
    let source = temporary.path().join("source");
    let coworker = source.join("COWORKER.toml");
    fs::write(&coworker, fs::read_to_string(&coworker).unwrap() + "[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = './CODEX.toml'\nas = 'codex'\n").unwrap();
    fs::write(source.join("CODEX.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/codex'\n").unwrap();
    fs::write(source.join("ROLE.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/codex'\n[adapters.params]\nentrypoint = './PROMPT.md'\nmodel = 'gpt-5'\nreasoning_effort = 'medium'\n").unwrap();
    fs::write(source.join("PROMPT.md"), "Original instructions").unwrap();
    let mut composition = LocalWorkspaceComposition::load(&path).unwrap();
    composition.runtimes[0].state_directory = temporary.path().join("private/one");
    let selected = &composition.runtimes[0];
    let prepared = selected.prepare_resident().unwrap();
    assert_eq!(
        prepared.fingerprint,
        selected.prepare_resident().unwrap().fingerprint
    );
    fs::write(source.join("PROMPT.md"), "Changed instructions").unwrap();
    assert_ne!(
        prepared.fingerprint,
        selected.prepare_resident().unwrap().fingerprint
    );
    let mut conflicting = prepared.clone();
    conflicting.acting_actor_ref = INTRINSIC_CWR_ACTOR_REF.into();
    assert!(
        conflicting
            .open("cwp://example.test/host", LocalRuntimeOptions::default())
            .err()
            .unwrap()
            .to_string()
            .contains("configured acting coworker")
    );
    assert!(!selected.state_directory.exists());
    let runtime = prepared
        .open("cwp://example.test/host", LocalRuntimeOptions::default())
        .unwrap();
    assert!(
        runtime
            .reload_graph()
            .unwrap_err()
            .to_string()
            .contains("until exit")
    );
}

#[test]
fn legacy_composition_keeps_runtime_identity_storage_and_fingerprint() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    let canonical = LocalWorkspaceComposition::load(&path).unwrap();
    let before = &canonical.runtimes[0];
    let fingerprint = before.prepare_resident().unwrap().fingerprint;
    let source = fs::read_to_string(&path).unwrap();
    fs::write(&path, source.replace("[[runtimes]]", "[[loops]]")).unwrap();
    let legacy = LocalWorkspaceComposition::load(&path).unwrap();
    let after = &legacy.runtimes[0];
    assert_eq!(before.runtime_ref, after.runtime_ref);
    assert_eq!(before.database_path(), after.database_path());
    assert_eq!(
        before.runtime_directory().unwrap(),
        after.runtime_directory().unwrap()
    );
    assert_eq!(fingerprint, after.prepare_resident().unwrap().fingerprint);
    fs::write(
        &path,
        source + &declaration("two").replace("[[runtimes]]", "[[loops]]"),
    )
    .unwrap();
    assert!(LocalWorkspaceComposition::load(&path).is_err());
}
