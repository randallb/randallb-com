use cwr::v2026_09::graph::{
    resolve_graph, resolve_graph_with_options, EmptyResourceResolver, GraphLoadOptions,
};
use std::{fs, path::Path};

fn fixture(root: &Path) {
    fs::create_dir_all(root.join("coworkers/helper")).unwrap();
    for (path, contents) in [
        ("CWR.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath = './coworkers/helper/COWORKER.toml'\n"),
        ("coworkers/helper/COWORKER.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Helper'\nslug = 'helper'\n[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = './integration.toml'\nas = 'local'\n"),
        ("coworkers/helper/ROLE.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/loopback'\n[[actions]]\nverb = 'echo'\nobject = 'message'\ntitle = 'Echo'\ndescription = 'Echo'\n[actions.input_schema]\npath = './schema.json'\n[actions.output_schema]\npath = './schema.json'\n"),
        ("coworkers/helper/integration.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/loopback'\n"),
        ("coworkers/helper/schema.json", r#"{"type":"object"}"#),
    ] { fs::write(root.join(path), contents).unwrap(); }
}

#[test]
fn declaration_location_does_not_select_execution_directory() {
    let source = tempfile::tempdir().unwrap();
    fixture(source.path());
    let graph = resolve_graph(source.path().join("CWR.toml")).unwrap();
    assert_eq!(
        graph.actor_integrations[0].working_path,
        std::env::current_dir().unwrap().canonicalize().unwrap()
    );
    assert_eq!(
        graph.routes[0].working_path,
        graph.actor_integrations[0].working_path
    );
    assert_eq!(
        graph.routes[0].process_adapter_route().unwrap(),
        graph.actor_integrations[0].process_adapter_route().unwrap()
    );
}

#[test]
fn workspace_selects_execution_directory_independently_of_source_roots() {
    let source = tempfile::tempdir().unwrap();
    fixture(source.path());
    let working = tempfile::tempdir().unwrap();
    let options = GraphLoadOptions {
        working_path: Some(working.path().into()),
        ..GraphLoadOptions::default()
    };
    let graph = resolve_graph_with_options(
        source.path().join("CWR.toml"),
        &EmptyResourceResolver,
        &options,
    )
    .unwrap();
    assert_eq!(
        graph.actor_integrations[0].working_path,
        working.path().canonicalize().unwrap()
    );
    assert_eq!(
        graph.routes[0].working_path,
        graph.actor_integrations[0].working_path
    );
    assert!(graph.actor_integrations[0]
        .coworker_source
        .starts_with(source.path().canonicalize().unwrap()));
}

#[test]
fn imported_loop_sources_keep_the_workspace_execution_directory() {
    let source = tempfile::tempdir().unwrap();
    let child = source.path().join("package");
    fixture(&child);
    fs::write(source.path().join("CWR.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath = './package/CWR.toml'\nas = 'package'\n").unwrap();
    let working = tempfile::tempdir().unwrap();
    let options = GraphLoadOptions {
        working_path: Some(working.path().into()),
        ..GraphLoadOptions::default()
    };
    let graph = resolve_graph_with_options(
        source.path().join("CWR.toml"),
        &EmptyResourceResolver,
        &options,
    )
    .unwrap();
    assert_eq!(graph.actors.len(), 1);
    assert_eq!(
        graph.routes[0].working_path,
        working.path().canonicalize().unwrap()
    );
}

#[test]
fn missing_or_file_execution_directories_reject_configuration() {
    let source = tempfile::tempdir().unwrap();
    fixture(source.path());
    for path in [
        source.path().join("missing"),
        source.path().join("CWR.toml"),
    ] {
        let options = GraphLoadOptions {
            working_path: Some(path),
            ..GraphLoadOptions::default()
        };
        assert!(resolve_graph_with_options(
            source.path().join("CWR.toml"),
            &EmptyResourceResolver,
            &options
        )
        .is_err());
    }
}
