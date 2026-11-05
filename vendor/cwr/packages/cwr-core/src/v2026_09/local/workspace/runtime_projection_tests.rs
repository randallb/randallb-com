use super::*;

#[test]
fn runtime_selection_accepts_generic_and_reusable_graphs_without_loop_semantics() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    let source_path = temporary.path().join("source/CWR.toml");
    let source = fs::read_to_string(&source_path).unwrap();
    for type_ref in [
        crate::v2026_09::graph::RUNTIME_TYPE,
        "cwp://org.coworkerprotocol/2026-09/schemas",
    ] {
        fs::write(
            &source_path,
            source.replace("cwp://org.coworkerprotocol/2026-09/schemas/loop", type_ref),
        )
        .unwrap();
        let composition = LocalWorkspaceComposition::load(&path).unwrap();
        composition.check().unwrap();
        let graph = composition.runtimes[0].resolve_graph().unwrap();
        assert_eq!(graph.type_ref, type_ref);
        assert_ne!(graph.reference, graph.runtime_ref);
        assert_eq!(graph.actors.len(), 1);
        assert!(graph.actors[0].default_conversation_target_ref.is_none());
    }
}

#[test]
fn declared_workspace_runtime_projects_standalone_gateway_routes() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("mail"));
    let source = temporary.path().join("source");
    fs::write(
        source.join("CWR.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/runtime'\n[[imports]]\npath = './gateway/GATEWAY.toml'\nas = 'mail'\n",
    )
    .unwrap();
    fs::create_dir_all(source.join("gateway/handler")).unwrap();
    fs::write(
        source.join("gateway/GATEWAY.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/gateway'\nrole = { ref = '~@/handler' }\nintegration = { ref = '~@/provider' }\naction_name = 'send--mail'\n[[imports]]\npath = './handler/ROLE.toml'\nas = 'handler'\n[[imports]]\npath = './provider.toml'\nas = 'provider'\n",
    )
    .unwrap();
    fs::write(
        source.join("gateway/handler/ROLE.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/codex'\n[adapters.params]\nentrypoint = './PROMPT.md'\nmodel = 'gpt-6-sol'\nreasoning_effort = 'medium'\n[[actions]]\nverb = 'send'\nobject = 'mail'\ntitle = 'Send mail'\ndescription = 'Send an email'\n[actions.input_schema]\npath = './schema.json'\n[actions.output_schema]\npath = './schema.json'\n",
    )
    .unwrap();
    fs::write(
        source.join("gateway/handler/schema.json"),
        "{\"type\":\"object\"}",
    )
    .unwrap();
    fs::write(
        source.join("gateway/handler/PROMPT.md"),
        "Handle gateway operations safely.",
    )
    .unwrap();
    fs::write(
        source.join("gateway/provider.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/codex'\n",
    )
    .unwrap();

    let composition = LocalWorkspaceComposition::load(path).unwrap();
    let graph = composition
        .configured_runtime("mail")
        .unwrap()
        .resolve_graph()
        .unwrap();
    assert!(graph.actors.is_empty());
    assert!(graph.actor_integrations.is_empty());
    assert!(graph.routes.is_empty());
    assert_eq!(graph.gateways.len(), 1);
    assert_eq!(graph.gateways[0].route.action_name, "send--mail");
}
