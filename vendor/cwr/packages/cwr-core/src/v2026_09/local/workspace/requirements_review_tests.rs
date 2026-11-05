use super::*;

#[test]
fn undeclared_workspace_bindings_cannot_be_forwarded_or_activated() {
    for declared in [false, true] {
        let dir = setup();
        let path = dir.path().join("ready.toml");
        fs::write(&path, fs::read_to_string(&path).unwrap() + &input("token", requirements::PLAINTEXT_TYPE)).unwrap();
        let declarations = if declared { input("team", requirements::PLAINTEXT_TYPE) } else { String::new() };
        let composition = workspace(dir.path(), &(declarations + "[bindings.team]\nvalue = 'private-undeclared-value'\n"), &(selected("ready") + "[[runtimes.inputs]]\nref = '~@/team'\nas = 'token'\n"));
        let report = composition.requirements().unwrap();
        let expected = if declared { WorkspaceReadiness::Ready } else { WorkspaceReadiness::Invalid };
        assert_eq!(report.readiness, expected, "{report:?}");
        assert_eq!(report.runtimes[0].readiness, expected, "{report:?}");
        assert!(!serde_json::to_string(&report).unwrap().contains("private-undeclared-value"));
        assert_eq!(composition.runtimes[0].resolve_graph().is_ok(), declared);
        if !declared {
            assert!(report.issues.iter().any(|issue| issue.message.contains("without a declaration")));
            assert!(composition.runtimes[0].selected_inputs(&BTreeSet::new()).is_err());
            assert!(composition.runtimes[0].open("cwp://example.test/cli").is_err());
        }
        assert!(!dir.path().join("state").exists());
    }
}

#[test]
fn environment_bindings_still_validate_startup_without_reading_values_or_opening_state() {
    for (required, available) in [(false, false), (false, true), (true, true)] {
        for fault in ["none", "acting_coworker", "duplicate_handle", "reserved_handle"] {
            let dir = setup();
            fs::write(
                dir.path().join("caller.toml"),
                "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Caller'\nslug = 'caller'\n",
            )
            .unwrap();
            let token = input("token", requirements::SECRET_TYPE) + &format!("required = {required}\n");
            fs::write(dir.path().join("ready.toml"), format!("type = 'cwp://org.coworkerprotocol/2026-09/schemas/loop'\n{token}[[imports]]\npath = './caller.toml'\n[[imports]]\npath = './person.toml'\n[imports.inputs]\ntoken = {{ input = 'token' }}\n")).unwrap();
            let slug = if fault == "reserved_handle" { "cwr" } else { "person" };
            fs::write(dir.path().join("person.toml"), format!("type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Person'\nslug = '{slug}'\n{token}[[integrations]]\npath = './integration.toml'\nas = 'fixture'\n[integrations.inputs]\ntoken = {{ input = 'token' }}\n")).unwrap();
            // Reading PATH would fail this schema instead of leaving the value unresolved.
            fs::write(dir.path().join("integration.toml"), format!("type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n{token}[adapter]\npath = './adapter.toml'\n[params]\ntoken = {{ input = 'token' }}\n")).unwrap();
            fs::write(dir.path().join("adapter.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/adapter'\nref = 'cwp://example.test/fixture'\nname = 'Fixture'\n[integration_params_schema]\npath = './params.json'\n").unwrap();
            fs::write(dir.path().join("params.json"), r#"{"type":"object","properties":{"token":{"const":"never-the-environment-value"}}}"#).unwrap();
            let selection = selected("ready") + "[runtimes.inputs]\ntoken = { env = 'PATH' }\n";
            let mut composition = workspace(dir.path(), "", &selection);
            match fault {
                "acting_coworker" => composition.runtimes[0].acting_coworker = Some("unknown".into()),
                "duplicate_handle" => {
                    fs::write(dir.path().join("other.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Other'\nslug = 'person'\n").unwrap();
                    let source = fs::read_to_string(dir.path().join("ready.toml")).unwrap();
                    fs::write(dir.path().join("ready.toml"), source + "[[imports]]\npath = './other.toml'\n").unwrap();
                }
                _ => {}
            }

            let names = if available { BTreeSet::from(["PATH".into()]) } else { BTreeSet::new() };
            let report = composition.requirements_with_environment(&names).unwrap();
            let expected = if fault == "none" { WorkspaceReadiness::Ready } else { WorkspaceReadiness::Invalid };
            assert_eq!(report.runtimes[0].readiness, expected, "{required}/{available}/{fault}: {report:?}");
            assert_eq!(report.readiness, expected);
            let wire = serde_json::to_string(&report).unwrap();
            assert!(!wire.contains("PATH"));
            assert!(!wire.contains("never-the-environment-value"));
            assert!(!dir.path().join("state").exists());
        }
    }
}

#[test]
fn unresolved_parameter_schemas_defer_static_sibling_validation() {
    for entrypoint in ["", "entrypoint = 'fixture'\n", "entrypoint = 'wrong'\n"] {
        let dir = setup();
        fs::write(
            dir.path().join("caller.toml"),
            "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Caller'\nslug = 'caller'\n",
        )
        .unwrap();
        let token = input("token", requirements::SECRET_TYPE) + "required = false\n";
        fs::write(dir.path().join("ready.toml"), format!("type = 'cwp://org.coworkerprotocol/2026-09/schemas/loop'\n{token}[[imports]]\npath = './caller.toml'\n[[imports]]\npath = './person.toml'\n[imports.inputs]\ntoken = {{ input = 'token' }}\n")).unwrap();
        fs::write(dir.path().join("person.toml"), format!("type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Person'\nslug = 'person'\n{token}[[integrations]]\npath = './integration.toml'\nas = 'fixture'\n[integrations.inputs]\ntoken = {{ input = 'token' }}\n")).unwrap();
        fs::write(dir.path().join("integration.toml"), format!("type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n{token}[adapter]\npath = './adapter.toml'\n[params]\n{entrypoint}token = {{ input = 'token' }}\n")).unwrap();
        fs::write(dir.path().join("adapter.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/adapter'\nref = 'cwp://example.test/fixture'\nname = 'Fixture'\n[integration_params_schema]\npath = './params.json'\n").unwrap();
        fs::write(dir.path().join("params.json"), r#"{"type":"object","required":["entrypoint"],"properties":{"entrypoint":{"const":"fixture"},"token":{"type":"string"}}}"#).unwrap();
        let composition = workspace(
            dir.path(),
            "",
            &(selected("ready") + "[runtimes.inputs]\ntoken = { env = 'PATH' }\n"),
        );
        let report = composition
            .requirements_with_environment(&BTreeSet::from(["PATH".into()]))
            .unwrap();
        assert_eq!(
            report.readiness,
            WorkspaceReadiness::Ready,
            "{entrypoint:?}: {report:?}"
        );
        assert!(!dir.path().join("state").exists());
    }
}

#[test]
fn invalid_and_unsupported_loops_take_precedence_over_partial_readiness() {
    for (declarations, selection, expected) in [
        ("".into(), "[[runtimes.inputs]]\nref = '~@/unknown'\nas = 'token'\n", WorkspaceReadiness::Invalid),
        (input("token", "cwp://example.test/unsupported") + "required = false\n", "", WorkspaceReadiness::Unsupported),
    ] {
        let dir = setup();
        let source = fs::read_to_string(dir.path().join("needs_secret.toml")).unwrap();
        fs::write(dir.path().join("needs_secret.toml"), source + &declarations).unwrap();
        let composition = workspace(dir.path(), "", &(selected("ready") + &selected("needs_secret") + selection));
        let report = composition.requirements().unwrap();
        assert_eq!(report.readiness, expected, "{report:?}");
        assert_eq!(report.runtimes[0].readiness, WorkspaceReadiness::Ready);
        assert_eq!(report.runtimes[1].readiness, expected);
        assert!(!dir.path().join("state").exists());
        composition.runtimes[0].open("cwp://example.test/cli").unwrap();
        assert!(!composition.runtimes[1].state_directory.exists());
    }
}

#[test]
fn typed_volume_rows_report_the_source_grant() {
    for (grant, mapping, source_name, expected_access, expected_status) in [
        ("read", "read_write", "files", Some("read"), RequirementStatus::Invalid),
        ("read_write", "read_write", "files", Some("read_write"), RequirementStatus::Invalid),
        ("read_write", "read", "files", Some("read_write"), RequirementStatus::Supplied),
        ("read", "read", "missing", None, RequirementStatus::Invalid),
    ] {
        let dir = setup();
        let source = fs::read_to_string(dir.path().join("ready.toml")).unwrap();
        fs::write(dir.path().join("ready.toml"), source + "[[volumes]]\nas = 'project'\naccess = 'read'\n").unwrap();
        workspace(dir.path(), "", &(selected("ready") + &format!("[[runtimes.volumes]]\nref = '~@/{source_name}'\nas = 'project'\naccess = '{mapping}'\n")));
        let path = dir.path().join("WORKSPACE.toml");
        let source = fs::read_to_string(&path).unwrap().replace("path = './volume'", &format!("path = './volume'\naccess = '{grant}'"));
        fs::write(&path, source).unwrap();
        let report = LocalWorkspaceComposition::load(path).unwrap().requirements().unwrap();
        let row = report.requirements.iter().find(|row| row.alias == "project").unwrap();
        assert_eq!(row.available_access.as_deref(), expected_access);
        assert_eq!(row.requested_access.as_deref(), Some("read"));
        assert_eq!(row.status, expected_status);
    }
}
