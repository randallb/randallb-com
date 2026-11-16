use super::*;

#[cfg(unix)]
#[test]
fn live_reload_rejects_retargeted_imported_volume_symlink() {
    let _lock = crate::workspace_environment::TEST_ENVIRONMENT_LOCK
        .lock()
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let _environment = super::handoff_tests::EnvironmentGuard::isolate(root.path());
    let path = root.path().join("CWR.toml");
    let loop_path = root.path().join("loop/CWR.toml");
    let first = root.path().join("first");
    let second = root.path().join("second");
    let alias = root.path().join("memory");
    fs::create_dir(root.path().join("loop")).unwrap();
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    std::os::unix::fs::symlink(&first, &alias).unwrap();
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
    write_imported_bindings(
        &path,
        json!({
            "version": 1,
            "bindings": [{"source": loop_path, "alias": "memory", "path": alias, "access": "read_write"}]
        }),
    );
    let graph = resolve_local_graph(&path).unwrap();
    let identity =
        LocalHostIdentity::new(graph.actors[0].actor_ref.clone(), "cwp://example.test/cli");
    let runtime =
        LocalRuntime::open_with_state_path(&path, root.path().join("state.sqlite3"), identity)
            .unwrap();
    fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&second, &alias).unwrap();
    assert!(runtime
        .reload_graph()
        .unwrap_err()
        .to_string()
        .contains("reopen"));
}

#[test]
fn approved_input_handoff_options_keep_imported_volume_bindings() {
    let _lock = crate::workspace_environment::TEST_ENVIRONMENT_LOCK
        .lock()
        .unwrap();
    let root = fixture();
    let _environment = super::handoff_tests::EnvironmentGuard::isolate(root.path());
    let path = root.path().join("CWR.toml");
    let loop_path = root.path().join("loop/CWR.toml");
    let memory = root.path().join("memory");
    fs::create_dir(root.path().join("loop")).unwrap();
    fs::create_dir(&memory).unwrap();
    fs::write(
        &loop_path,
        format!(
            "type = '{}'\n[[volumes]]\nas = 'memory'\naccess = 'read_write'\n",
            graph::RUNTIME_TYPE
        ),
    )
    .unwrap();
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap() + "[[imports]]\npath = './loop/CWR.toml'\nas = 'loop'\n",
    )
    .unwrap();
    write_imported_bindings(
        &path,
        json!({
            "version": 1,
            "bindings": [{"source": loop_path, "alias": "memory", "path": memory, "access": "read_write", "origin": "registry"}]
        }),
    );
    let target = connection_target(&path).unwrap();
    let options = options_for_target(&target).unwrap();
    assert_eq!(options.imported_volumes.len(), 1);
    graph::resolve_graph_with_options(&path, &graph::EmptyResourceResolver, &options).unwrap();
}

#[test]
fn configured_workspace_loads_nested_imported_volume_binding() {
    let _lock = crate::workspace_environment::TEST_ENVIRONMENT_LOCK
        .lock()
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let _environment = super::handoff_tests::EnvironmentGuard::isolate(root.path());
    let parent = root.path().join("CWR.toml");
    let child = root.path().join("loop/CWR.toml");
    let memory = root.path().join("memory");
    fs::create_dir_all(child.parent().unwrap()).unwrap();
    fs::create_dir(&memory).unwrap();
    fs::write(
        &parent,
        format!(
            "type = '{}'\n[[imports]]\npath = './loop/CWR.toml'\nas = 'loop'\n",
            graph::RUNTIME_TYPE
        ),
    )
    .unwrap();
    fs::write(&child, format!(
        "type = '{}'\n[[volumes]]\nas = 'customer'\naccess = 'read_write'\n[[imports]]\npath = './person.toml'\nas = 'person'\n",
        graph::RUNTIME_TYPE
    )).unwrap();
    fs::write(root.path().join("loop/person.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Person'\nslug = 'person'\n").unwrap();
    fs::write(root.path().join("WORKSPACE.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/workspace'\n[[volumes]]\nname = 'files'\npath = './memory'\n[[runtimes]]\nname = 'parent'\npath = './CWR.toml'\nsource_roots = ['.']\nworking_directory = '.'\nvolume = 'files'\n").unwrap();
    write_imported_bindings(
        &parent,
        json!({
            "version": 1,
            "bindings": [{"source": child, "alias": "customer", "path": memory, "access": "read_write", "origin": "registry"}]
        }),
    );
    let composition = LocalWorkspaceComposition::load(root.path().join("WORKSPACE.toml")).unwrap();
    assert_eq!(
        composition.runtimes[0]
            .graph_options_with_imported_volumes()
            .unwrap()
            .imported_volumes
            .len(),
        1
    );
    let graph = composition.runtimes[0].resolve_graph().unwrap();
    assert_eq!(graph.actors.len(), 1);
    assert_eq!(
        composition.requirements().unwrap().readiness,
        WorkspaceReadiness::Ready
    );
}
