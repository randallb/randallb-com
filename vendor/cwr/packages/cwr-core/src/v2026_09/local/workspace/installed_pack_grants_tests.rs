use super::*;

#[test]
fn installed_design_specialists_discard_host_action_grants_in_configured_graph() {
    fn copy_tree(source: &Path, target: &Path) {
        fs::create_dir_all(target).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target.join(entry.file_name()));
            } else {
                fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../wlpacks/design/source");
    copy_tree(&source, &root.join("loops/design"));
    fs::create_dir(root.join("private")).unwrap();
    fs::write(root.join("CWR.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/runtime'\n[[imports]]\npath = './loops/design/CWR.toml'\nas = 'design'\n[imports.inputs]\npermitted_actions = ['~@/ui_ux_designer', '~@/content_strategist']\n").unwrap();
    let manifest = root.join("WORKSPACE.toml");
    fs::write(&manifest, "type = 'cwp://org.coworkerprotocol/2026-09/schemas/workspace'\n[[volumes]]\nname = 'files'\npath = './private'\n[[runtimes]]\nname = 'parent'\npath = './CWR.toml'\nsource_roots = ['.']\nworking_directory = '.'\nvolume = 'files'\n").unwrap();
    let composition = LocalWorkspaceComposition::load(&manifest).unwrap();
    let configured = composition.configured_runtime("parent").unwrap();
    let mut options = configured.graph_options();
    options
        .imported_volumes
        .push(crate::v2026_09::graph::ImportedVolumeBinding {
            source: fs::canonicalize(root.join("loops/design/CWR.toml")).unwrap(),
            alias: "customer".into(),
            path: fs::canonicalize(root.join("private")).unwrap(),
            access: "read_write".into(),
        });
    let graph = crate::v2026_09::graph::resolve_graph_with_options(
        &configured.cwr_toml,
        &crate::v2026_09::graph::EmptyResourceResolver,
        &options,
    )
    .unwrap();
    let manager = graph
        .actors
        .iter()
        .find(|actor| {
            actor
                .coworker_source
                .ends_with("coworkers/design_crit_coordinator/COWORKER.toml")
        })
        .unwrap_or_else(|| {
            panic!(
                "resolved actors: {:?}; bindings: {:?}",
                graph.actors, graph.actor_integrations
            )
        });
    let manager_binding = graph
        .actor_integrations
        .iter()
        .find(|binding| binding.actor_ref == manager.actor_ref)
        .unwrap();
    assert_eq!(
        manager_binding.actor_integration_params["cwr_action_actor_refs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for slug in ["ui_ux_designer", "content_strategist"] {
        let actor = graph
            .actors
            .iter()
            .find(|actor| {
                actor
                    .coworker_source
                    .ends_with(format!("coworkers/{slug}/COWORKER.toml"))
            })
            .unwrap();
        let binding = graph
            .actor_integrations
            .iter()
            .find(|binding| binding.actor_ref == actor.actor_ref)
            .unwrap();
        assert_eq!(
            binding.actor_integration_params["cwr_action_actor_refs"],
            serde_json::json!([])
        );
        assert_eq!(binding.actor_integration_params["cwr_only"], true);
    }
}
