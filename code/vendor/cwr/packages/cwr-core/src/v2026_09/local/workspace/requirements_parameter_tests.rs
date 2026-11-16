use super::*;

pub(super) fn parameter_workspace(
    params: &str,
    schema: &Value,
) -> (tempfile::TempDir, LocalWorkspaceComposition) {
    let dir = setup();
    let declaration = input("token", requirements::SECRET_TYPE) + "required = false\n";
    fs::write(
        dir.path().join("caller.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Caller'\nslug = 'caller'\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("ready.toml"),
        format!(
            "type = 'cwp://org.coworkerprotocol/2026-09/schemas/loop'\n{declaration}[[imports]]\npath = './caller.toml'\n[[imports]]\npath = './person.toml'\n[imports.inputs]\ntoken = {{ input = 'token' }}\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.path().join("person.toml"),
        format!(
            "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Person'\nslug = 'person'\n{declaration}[[integrations]]\npath = './integration.toml'\nas = 'fixture'\n[integrations.inputs]\ntoken = {{ input = 'token' }}\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.path().join("integration.toml"),
        format!(
            "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n{params}\n{declaration}[adapter]\npath = './adapter.toml'\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.path().join("adapter.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/adapter'\nref = 'cwp://example.test/fixture'\nname = 'Fixture'\n[integration_params_schema]\npath = './params.json'\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("params.json"),
        serde_json::to_vec(schema).unwrap(),
    )
    .unwrap();
    let composition = workspace(
        dir.path(),
        "",
        &(selected("ready") + "[runtimes.inputs]\ntoken = { env = 'PATH' }\n"),
    );
    (dir, composition)
}

fn assert_readiness(composition: &LocalWorkspaceComposition, expected: WorkspaceReadiness) {
    let report = composition
        .requirements_with_environment(&BTreeSet::from(["PATH".into()]))
        .unwrap();
    assert_eq!(report.readiness, expected, "{report:?}");
    assert_eq!(report.runtimes[0].readiness, expected, "{report:?}");
    let wire = serde_json::to_string(&report).unwrap();
    assert!(!wire.contains("PATH"));
    assert!(!composition.runtimes[0].state_directory.exists());
}

// Runtime startup preserves the independent Caller while removing every route
// belonging to an invalid Person. Setup inspection remains strict above.
pub(super) fn assert_execution_configuration(composition: &LocalWorkspaceComposition, ready: bool) {
    let graph = composition.runtimes[0].resolve_graph().unwrap();
    assert!(graph.actors.iter().any(|actor| actor.coworker_slug == "caller"));
    assert_eq!(graph.actors.iter().any(|actor| actor.coworker_slug == "person"), ready);
    assert_eq!(!graph.actor_integrations.is_empty(), ready);
    assert_eq!(graph.warnings.is_empty(), ready);
    if !ready { assert!(graph.routes.is_empty()); }
}

#[test]
fn unread_parameter_schemas_defer_until_values_are_concrete() {
    let (dir, mut composition) = parameter_workspace(
        "params = { token = { input = 'token' } }",
        &json!({"type":"object","properties":{"token":{"const":"allowed"}}}),
    );

    assert_readiness(&composition, WorkspaceReadiness::Ready);

    composition.runtimes[0].inputs = json!({"token":"allowed"});
    assert_readiness(&composition, WorkspaceReadiness::Ready);
    composition.runtimes[0].resolve_graph().unwrap();

    composition.runtimes[0].inputs = json!({"token":"rejected"});
    assert_readiness(&composition, WorkspaceReadiness::Invalid);
    assert_execution_configuration(&composition, false);
    let warnings = composition.runtimes[0].resolve_graph().unwrap().warnings;
    assert!(!serde_json::to_string(&warnings).unwrap().contains("rejected"));
    assert!(!dir.path().join("state").exists());
}

#[test]
fn unread_parameters_still_reject_invalid_adapter_schemas() {
    let (_dir, composition) = parameter_workspace(
        "params = { token = { input = 'token' } }",
        &json!({"type":"not-a-json-schema-type"}),
    );
    assert_readiness(&composition, WorkspaceReadiness::Invalid);
    assert_execution_configuration(&composition, false);
}

#[test]
fn concrete_parameters_are_validated_during_inspection_and_execution() {
    let (dir, composition) = parameter_workspace(
        "params = { token = 'wrong' }",
        &json!({"type":"object","properties":{"token":{"const":"allowed"}}}),
    );
    assert_readiness(&composition, WorkspaceReadiness::Invalid);
    assert_execution_configuration(&composition, false);
    let warnings = composition.runtimes[0].resolve_graph().unwrap().warnings;
    assert!(!serde_json::to_string(&warnings).unwrap().contains("wrong"));
    assert!(!dir.path().join("state").exists());
}

#[test]
fn non_schema_parameter_checks_still_apply_to_unread_values() {
    for (value, expected) in [
        ("{ input = 'token' }", WorkspaceReadiness::Invalid),
        ("[{ input = 'token' }]", WorkspaceReadiness::Ready),
    ] {
        let (dir, mut composition) = parameter_workspace("params = {}", &json!({}));
        let person = dir.path().join("person.toml");
        let source = fs::read_to_string(&person).unwrap().replace(
            "[integrations.inputs]",
            &format!("params = {{ cwr_action_actor_refs = {value} }}\n[integrations.inputs]"),
        );
        fs::write(person, source).unwrap();

        assert_readiness(&composition, expected);
        composition.runtimes[0].inputs = json!({"token":"cwp://!cwr/actors/cwr"});
        assert_execution_configuration(&composition, expected == WorkspaceReadiness::Ready);
    }
}

#[test]
fn absent_optional_whole_parameters_remain_omitted() {
    for required_property in [false, true] {
        let schema = if required_property {
            json!({"type":"object","required":["entrypoint"]})
        } else {
            json!({"type":"object"})
        };
        let (dir, mut composition) =
            parameter_workspace("params = { input = 'token' }", &schema);
        composition.runtimes[0].inputs = json!({});

        let report = composition.requirements().unwrap();
        let expected = if required_property {
            WorkspaceReadiness::Invalid
        } else {
            WorkspaceReadiness::Ready
        };
        assert_eq!(report.readiness, expected, "{report:?}");
        assert_execution_configuration(&composition, !required_property);
        assert!(!dir.path().join("state").exists());
    }
}
