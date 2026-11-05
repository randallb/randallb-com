use std::{fs, path::Path};

#[test]
fn workspace_setup_projects_and_round_trips_public_fields() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/contracts/agent-recovery-turn/workspace/CWR.toml");
    let workspace = inspect_local_workspace(&fixture).expect("inspect local workspace");
    assert_eq!(workspace.workspace_ref, crate::v2026_09::test_fixture::graph().reference);
    assert_eq!(
        workspace.runtime_ref,
        crate::v2026_09::test_fixture::graph().runtime_ref
    );
    assert_eq!(workspace.actors.len(), 2);
    assert!(workspace
        .actors
        .iter()
        .any(|actor| actor.actor_ref == crate::v2026_09::test_fixture::actor_ref("person")
            && actor.handle == "@person"
            && actor.name == "Person"
            && actor.category == Some(crate::v2026_09::graph::CoworkerCategory::Human)));
    assert!(workspace
        .actors
        .iter()
        .any(|actor| actor.actor_ref == crate::v2026_09::test_fixture::actor_ref("helper")
            && actor.category == Some(crate::v2026_09::graph::CoworkerCategory::Ai)));

    let inspect = HostCommand::InspectWorkspace {
        version: HOST_COMMAND_VERSION,
        request_id: "workloop-inspect-1".into(),
    };
    inspect.validate().expect("validate inspection command");
    let inspect_wire = serde_json::to_value(&inspect).expect("serialize inspection command");
    assert_eq!(
        inspect_wire,
        json!({
            "kind": "inspect_workspace",
            "version": 1,
            "request_id": "workloop-inspect-1"
        })
    );
    assert_eq!(
        serde_json::from_value::<HostCommand>(inspect_wire).expect("decode inspection command"),
        inspect
    );

    let inspected = HostEvent::WorkspaceInspected {
        request_id: "workloop-inspect-1".into(),
        workspace: workspace.clone(),
        default_actor_ref: Some(crate::v2026_09::test_fixture::actor_ref("person").into()),
    };
    inspected.validate().expect("validate inspected workspace");
    let inspected_wire = serde_json::to_value(&inspected).expect("serialize inspected workspace");
    assert_eq!(inspected_wire["kind"], "workspace_inspected");
    assert_eq!(
        serde_json::from_value::<HostEvent>(inspected_wire).expect("decode inspected workspace"),
        inspected
    );

    let select = HostCommand::SelectActor {
        version: HOST_COMMAND_VERSION,
        request_id: "workloop-select-1".into(),
        actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
    };
    select.validate().expect("validate actor selection");
    let select_wire = serde_json::to_value(&select).expect("serialize actor selection");
    assert_eq!(select_wire["kind"], "select_actor");
    assert_eq!(
        serde_json::from_value::<HostCommand>(select_wire).expect("decode actor selection"),
        select
    );

    let mut local_catalog =
        crate::v2026_09::local::load_local_catalog(&fixture, crate::v2026_09::test_fixture::actor_ref("person"))
            .expect("load selected catalog");
    local_catalog
        .actions
        .push(crate::v2026_09::local::LocalAction {
            activation: "@helper--answer".into(),
            actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
            actor_integration_ref: crate::v2026_09::test_fixture::binding_ref("helper").into(),
            action_name: "answer".into(),
            title: "Answer".into(),
            description: "Answer a question".into(),
            input_schema: json!(true),
            output_schema: json!(false),
        });
    local_catalog.retained_actors = vec![local_catalog.acting_actor.clone()];
    let projected_catalog = project_host_catalog(&local_catalog);
    assert_eq!(projected_catalog.retained_actors, local_catalog.retained_actors.iter().map(project_host_actor).collect::<Vec<_>>());
    assert_eq!(projected_catalog.known_actors, local_catalog.known_actors.iter().map(project_host_actor).collect::<Vec<_>>());
    assert!(projected_catalog.known_actors.iter().any(|actor| actor.handle == "@person"));
    let selected = HostEvent::ActorSelected {
        request_id: "workloop-select-1".into(),
        actor: project_host_actor(&local_catalog.acting_actor),
        catalog: project_host_catalog(&local_catalog),
    };
    selected.validate().expect("validate selected catalog");
    let selected_wire = serde_json::to_value(&selected).expect("serialize selected catalog");
    assert_eq!(selected_wire["kind"], "actor_selected");
    assert_eq!(selected_wire["catalog"]["actions"][0]["input_schema"], true);
    assert_eq!(
        selected_wire["catalog"]["actions"][0]["output_schema"],
        false
    );
    assert!(serde_json::to_string(&selected_wire)
        .expect("encode selected catalog")
        .find("actor_integration_ref")
        .is_none());
    assert_eq!(
        serde_json::from_value::<HostEvent>(selected_wire).expect("decode selected catalog"),
        selected
    );

    let updated = HostEvent::CatalogUpdated {
        actor: project_host_actor(&local_catalog.acting_actor),
        catalog: project_host_catalog(&local_catalog),
    };
    updated.validate().expect("validate catalog update");
    let updated_wire = serde_json::to_value(&updated).expect("serialize catalog update");
    assert_eq!(updated_wire["kind"], "catalog_updated");
    assert_eq!(
        serde_json::from_value::<HostEvent>(updated_wire).expect("decode catalog update"),
        updated
    );
}

#[test]
fn workspace_setup_validation_rejects_malformed_public_data() {
    let blank_selection = HostCommand::SelectActor {
        version: HOST_COMMAND_VERSION,
        request_id: "select-1".into(),
        actor_ref: " ".into(),
    };
    assert!(blank_selection.validate().is_err());

    let actor = HostActor {
        actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
        coworker_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
        handle: "@person".into(),
        name: "Person".into(),
        category: None,
        provider_model: None,
    };
    let duplicate = HostEvent::WorkspaceInspected {
        request_id: "inspect-1".into(),
        default_actor_ref: Some(crate::v2026_09::test_fixture::actor_ref("person").into()),
        workspace: HostWorkspace {
            warnings: Vec::new(),
            workspace_ref: "~@".into(),
            runtime_ref: "cwp://test.workloop/loops/configured".into(),
            actors: vec![actor.clone(), actor],
        },
    };
    assert!(duplicate
        .validate()
        .expect_err("reject duplicate workspace actors")
        .to_string()
        .contains("repeats actor"));

    let orphaned_action = HostEvent::ActorSelected {
        request_id: "select-1".into(),
        actor: HostActor {
            actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
            coworker_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
            handle: "@person".into(),
            name: "Person".into(),
            category: None,
            provider_model: None,
        },
        catalog: HostActionCatalog {
                known_actors: Vec::new(),
                retained_actors: Vec::new(),
            default_conversation_target_ref: None,
            actors: Vec::new(),
            actions: vec![HostAction {
                activation: "@helper--answer".into(),
                actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
                action_name: "answer".into(),
                title: "Answer".into(),
                description: "Answer a question".into(),
                input_schema: json!({}),
                output_schema: json!({}),
            }],
        },
    };
    assert!(orphaned_action
        .validate()
        .expect_err("reject action outside catalog")
        .to_string()
        .contains("outside its catalog"));
}

#[test]
fn workspace_inspection_reports_disabled_dependencies_before_actor_selection() {
    let temporary = tempfile::tempdir().expect("temporary workspace");
    let manifest = temporary.path().join("CWR.toml");
    fs::write(
        &manifest,
        r#"
type = "cwp://org.coworkerprotocol/2026-09/schemas"

[[imports]]
path = "./missing.toml"
"#,
    )
    .expect("write invalid workspace");

    let workspace = inspect_local_workspace(&manifest).expect("inspect remaining workspace");
    assert!(workspace.actors.is_empty());
    assert_eq!(workspace.warnings.len(), 1);
    assert!(workspace.warnings[0].source.ends_with("missing.toml"));
}

#[test]
fn desktop_default_workspace_loads_without_repository_coworkers() {
    fn copy_tree(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/workloop-desktop/default-workspace");
    let isolated = tempfile::tempdir().unwrap();
    copy_tree(&source, isolated.path());
    let manifest = isolated.path().join("CWR.toml");
    let workspace = inspect_local_workspace(&manifest).expect("desktop workspace validates");
    assert_eq!(workspace.actors.len(), 2);
    for actor in ["root", "loop_studio"] {
        crate::v2026_09::local::load_local_catalog(
            &manifest,
            &workspace.actors.iter().find(|candidate| candidate.handle == format!("@{actor}")).unwrap().actor_ref,
        )
        .expect("desktop actor catalog resolves all references and schemas");
    }
}
