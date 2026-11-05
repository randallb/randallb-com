use super::*;

fn input(alias: &str, kind: &str) -> String {
    format!("[[inputs]]\nas = '{alias}'\ntype = '{kind}'\n")
}
fn setup() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("volume")).unwrap();
    fs::write(dir.path().join("person.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Person'\nslug = 'person'\n").unwrap();
    for name in ["ready", "needs_secret"] {
        fs::write(dir.path().join(format!("{name}.toml")), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/loop'\n[[imports]]\npath = './person.toml'\n").unwrap();
    }
    dir
}
fn selected(name: &str) -> String {
    format!("[[runtimes]]\nname = '{name}'\npath = './{name}.toml'\nsource_roots = ['.']\nworking_directory = '.'\nvolume = 'files'\n")
}
fn workspace(dir: &Path, declarations: &str, selections: &str) -> LocalWorkspaceComposition {
    let path = dir.join("WORKSPACE.toml");
    fs::write(&path, format!("type = 'cwp://org.coworkerprotocol/2026-09/schemas/workspace'\n{declarations}[[volumes]]\nname = 'files'\npath = './volume'\n{selections}")).unwrap();
    let mut result = LocalWorkspaceComposition::load(path).unwrap();
    for configured in &mut result.runtimes {
        configured.state_directory = dir.join("state").join(&configured.name);
    }
    result
}

#[test]
fn requirements_keep_ready_loop_usable_when_another_loop_is_unbound() {
    let dir = setup();
    let child = dir.path().join("needs_secret.toml");
    let source = fs::read_to_string(&child).unwrap();
    fs::write(child, source + &input("token", requirements::SECRET_TYPE)).unwrap();
    let declarations = input("team_token", requirements::SECRET_TYPE)
        + "[bindings.team_token]\nenv = 'CWR_TYPED_TEST_UNBOUND_TOKEN'\n";
    let selections = selected("ready")
        + &selected("needs_secret")
        + "[[runtimes.inputs]]\nref = '~@/team_token'\nas = 'token'\n";
    let composition = workspace(dir.path(), &declarations, &selections);
    let report = composition.requirements().unwrap();
    assert_eq!(report.readiness, WorkspaceReadiness::Partial);
    assert_eq!(report.runtimes[0].readiness, WorkspaceReadiness::Ready);
    assert_eq!(report.runtimes[1].readiness, WorkspaceReadiness::NeedsSetup);
    let consuming = report
        .requirements
        .iter()
        .find(|row| row.alias == "token")
        .unwrap();
    assert_eq!(consuming.sources[0].alias, "team_token");
    assert_eq!(consuming.runtime_name.as_deref(), Some("needs_secret"));
    let wire = serde_json::to_string(&report).unwrap();
    assert!(!wire.contains("CWR_TYPED_TEST_UNBOUND_TOKEN"));
    assert!(!dir.path().join("state").exists());
    let runtime = composition.runtimes[0].open("cwp://example.test/cli").unwrap();
    assert!(!runtime.catalog().unwrap().known_actors.is_empty());
    assert!(composition.runtimes[1].open("cwp://example.test/cli").is_err());
    assert!(!composition.runtimes[1].state_directory.exists());
    // Explicit availability metadata changes readiness without resolving the environment.
    let available = BTreeSet::from(["CWR_TYPED_TEST_UNBOUND_TOKEN".into()]);
    let report = composition
        .requirements_with_environment(&available)
        .unwrap();
    assert_eq!(report.readiness, WorkspaceReadiness::Ready);
    assert!(!composition.runtimes[1].state_directory.exists());
}

#[test]
fn typed_workspace_inputs_stay_distinct_and_optional_inputs_do_not_block() {
    let dir = setup();
    let source = fs::read_to_string(dir.path().join("ready.toml")).unwrap();
    fs::write(
        dir.path().join("ready.toml"),
        source
            + &input("a", requirements::PLAINTEXT_TYPE)
            + &input("b", requirements::PLAINTEXT_TYPE)
            + "required = false\n",
    )
    .unwrap();
    let declarations = input("first", requirements::PLAINTEXT_TYPE)
        + &input("second", requirements::PLAINTEXT_TYPE)
        + "required = false\n[bindings.first]\nvalue = { input = 'ordinary-object' }\n";
    let selection = selected("ready") + "[[runtimes.inputs]]\nref = '~@/first'\nas = 'a'\n[[runtimes.inputs]]\nref = '~@/second'\nas = 'b'\n";
    let composition = workspace(dir.path(), &declarations, &selection);
    let report = composition.requirements().unwrap();
    assert_eq!(report.readiness, WorkspaceReadiness::Ready, "{report:?}");
    let bindings = composition.runtimes[0]
        .selected_inputs(&BTreeSet::new())
        .unwrap();
    assert_eq!(
        bindings["a"].value,
        Some(json!({"input":"ordinary-object"}))
    );
    assert!(bindings["b"].value.is_none());
    composition.runtimes[0].open("cwp://example.test/cli").unwrap();
}

#[test]
fn invalid_input_mappings_are_configuration_errors_and_never_open_state() {
    for mapping in [
        "[[runtimes.inputs]]\nref = '~@/unknown'\nas = 'token'\n",
        "[[runtimes.inputs]]\nref = '~@/team'\nas = 'other'\n",
        "[[runtimes.inputs]]\nref = '~@/team'\nas = 'token'\n[[runtimes.inputs]]\nref = '~@/team'\nas = 'token'\n",
        "inputs = []\n",
    ] {
        let dir = setup();
        let source = fs::read_to_string(dir.path().join("ready.toml")).unwrap();
        fs::write(dir.path().join("ready.toml"), source + &input("token",requirements::SECRET_TYPE)).unwrap();
        let composition = workspace(dir.path(), &input("team",requirements::SECRET_TYPE), &(selected("ready") + mapping));
        let report = composition.requirements().unwrap();
        assert_eq!(report.runtimes[0].readiness,WorkspaceReadiness::Invalid,"{mapping}: {report:?}");
        assert!(composition.runtimes[0].open("cwp://example.test/cli").is_err());
        assert!(!dir.path().join("state").exists());
    }
}

#[test]
fn volume_mappings_preserve_grants_and_missing_folders_do_not_stop_discovery() {
    let dir = setup();
    let source = fs::read_to_string(dir.path().join("ready.toml")).unwrap();
    fs::write(
        dir.path().join("ready.toml"),
        source + "[[volumes]]\nas = 'project'\naccess = 'read_write'\n",
    )
    .unwrap();
    let composition = workspace(
        dir.path(),
        "",
        &(selected("ready")
            + "[[runtimes.volumes]]\nref = '~@/files'\nas = 'project'\naccess = 'read'\n"),
    );
    let report = composition.requirements().unwrap();
    assert_eq!(report.runtimes[0].readiness, WorkspaceReadiness::Invalid);
    assert!(composition.runtimes[0].open("cwp://example.test/cli").is_err());
    assert!(!dir.path().join("state").exists());
    fs::remove_dir(dir.path().join("volume")).unwrap();
    let path = dir.path().join("WORKSPACE.toml");
    let source = fs::read_to_string(&path)
        .unwrap()
        .replace("access = 'read'", "access = 'read_write'");
    fs::write(&path, source).unwrap();
    let report = LocalWorkspaceComposition::load(&path)
        .unwrap()
        .requirements()
        .unwrap();
    assert_eq!(report.runtimes[0].readiness, WorkspaceReadiness::NeedsSetup);
    assert!(
        report
            .requirements
            .iter()
            .any(|row| row.alias == "project"
                && row.requested_access.as_deref() == Some("read_write"))
    );
}

#[test]
fn legacy_bound_values_still_execute_but_inspection_reports_the_boundary() {
    let dir = setup();
    let source = fs::read_to_string(dir.path().join("ready.toml")).unwrap();
    fs::write(
        dir.path().join("ready.toml"),
        source + "[input_schema]\npath = './input.json'\n",
    )
    .unwrap();
    fs::write(dir.path().join("input.json"), r#"{"anyOf":[{"type":"object","required":["legacy"],"properties":{"legacy":{"const":"private-value"}}}]}"#).unwrap();
    let composition = workspace(
        dir.path(),
        "",
        &(selected("ready") + "[runtimes.inputs]\nlegacy = 'private-value'\n"),
    );
    let report = composition.requirements().unwrap();
    assert_eq!(report.readiness, WorkspaceReadiness::Unsupported);
    assert!(!serde_json::to_string(&report)
        .unwrap()
        .contains("private-value"));
    composition.runtimes[0].open("cwp://example.test/cli").unwrap();
}

#[test]
fn reload_cannot_reclassify_an_activated_secret_or_increase_volume_requirements() {
    let dir = setup();
    let source = fs::read_to_string(dir.path().join("ready.toml")).unwrap();
    let typed = source + &input("token", requirements::SECRET_TYPE);
    fs::write(dir.path().join("ready.toml"), &typed).unwrap();
    let composition = workspace(
        dir.path(),
        "",
        &(selected("ready") + "[runtimes.inputs]\ntoken = 'private-reload-value'\n"),
    );
    let runtime = composition.runtimes[0].open("cwp://example.test/cli").unwrap();
    fs::write(
        dir.path().join("ready.toml"),
        typed.replace(requirements::SECRET_TYPE, requirements::PLAINTEXT_TYPE),
    )
    .unwrap();
    let error = runtime.reload_graph().unwrap_err().to_string();
    assert!(error.contains("preserve"), "{error}");
    assert!(!error.contains("private-reload-value"));
    fs::write(
        dir.path().join("ready.toml"),
        typed + "[[volumes]]\nas = 'unapproved'\naccess = 'read_write'\n",
    )
    .unwrap();
    assert!(runtime
        .reload_graph()
        .unwrap_err()
        .to_string()
        .contains("validate the grants"));
}

#[path = "requirements_review_tests.rs"]
mod review;

#[path = "requirements_parameter_tests.rs"]
mod parameters;

#[path = "requirements_followup_tests.rs"]
mod followup;
