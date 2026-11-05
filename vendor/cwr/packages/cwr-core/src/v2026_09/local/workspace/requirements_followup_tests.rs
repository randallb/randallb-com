use super::*;

const LEFT_ENV: &str = "CWR_INSPECTION_ONLY_LEFT";
const RIGHT_ENV: &str = "CWR_INSPECTION_ONLY_RIGHT";

fn role_workspace(
    first: &str,
    second: Option<(&str, &str)>,
    binding: &str,
) -> (tempfile::TempDir, LocalWorkspaceComposition) {
    role_workspace_with_type(first, second, binding, requirements::PLAINTEXT_TYPE)
}

fn role_workspace_with_type(
    first: &str,
    second: Option<(&str, &str)>,
    binding: &str,
    type_ref: &str,
) -> (tempfile::TempDir, LocalWorkspaceComposition) {
    let (dir, mut composition) = parameters::parameter_workspace("", &json!({"type":"object"}));
    let declarations = input("left", type_ref) + &input("right", type_ref);
    fs::write(dir.path().join("ready.toml"), format!(
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/loop'\n{declarations}[[imports]]\npath = './caller.toml'\n[[imports]]\npath = './person.toml'\n[imports.inputs]\nleft = {{ input = 'left' }}\nright = {{ input = 'right' }}\n"
    )).unwrap();
    let mut person = format!(
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Person'\nslug = 'person'\n{declarations}[[integrations]]\npath = './integration.toml'\nas = 'fixture'\n{binding}\n"
    );
    for (index, (params, source)) in std::iter::once((first, "left")).chain(second).enumerate() {
        person += &format!("[[roles]]\npath = './role{index}.toml'\n[roles.inputs]\nvalue = {{ input = '{source}' }}\n");
        fs::write(dir.path().join(format!("role{index}.toml")), format!(
            "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n{}[[adapters]]\npath = './adapter.toml'\n{params}\n",
            input("value", type_ref)
        )).unwrap();
    }
    fs::write(dir.path().join("person.toml"), person).unwrap();
    fs::write(dir.path().join("adapter.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/adapter'\nref = 'cwp://example.test/fixture'\nname = 'Fixture'\n[coworker_params_schema]\npath = './params.json'\n").unwrap();
    composition.runtimes[0].inputs = json!({"left":{"env":LEFT_ENV}, "right":{"env":RIGHT_ENV}});
    (dir, composition)
}

fn role_readiness(composition: &LocalWorkspaceComposition, expected: WorkspaceReadiness) {
    let report = composition.requirements_with_environment(
        &BTreeSet::from([LEFT_ENV.into(), RIGHT_ENV.into()]),
    ).unwrap();
    assert_eq!(report.readiness, expected, "{report:?}");
    assert_eq!(report.runtimes[0].readiness, expected, "{report:?}");
    let serialized = serde_json::to_string(&report).unwrap();
    assert!(!serialized.contains(LEFT_ENV));
    assert!(!serialized.contains(RIGHT_ENV));
    assert!(!serialized.contains("private-role-value"));
    assert!(!composition.runtimes[0].state_directory.exists());
}

#[test]
fn shared_role_parameters_preserve_unread_source_identity() {
    for params in [
        "params = { shared = { input = 'value' } }",
        "params = { shared = { 'a/b~c' = [{ input = 'value' }], known = 'same' } }",
        "params = { input = 'value' }",
    ] {
        for source in ["left", "right"] {
            let (_dir, mut composition) = role_workspace(params, Some((params, source)), "");
            // A whole environment value is text, even for a plaintext declaration.
            let expected = if params == "params = { input = 'value' }" { WorkspaceReadiness::Invalid }
                else if source == "left" { WorkspaceReadiness::Ready } else { WorkspaceReadiness::Unsupported };
            role_readiness(&composition, expected);
            // Different aliases with equal concrete values still compose normally.
            composition.runtimes[0].inputs = json!({"left":{"shared":"equal"}, "right":{"shared":"equal"}});
            role_readiness(&composition, WorkspaceReadiness::Ready);
            composition.runtimes[0].resolve_graph().unwrap();
            composition.runtimes[0].inputs["right"] = json!({"shared":"different"});
            let expected = if source == "left" { WorkspaceReadiness::Ready } else { WorkspaceReadiness::Invalid };
            role_readiness(&composition, expected);
            parameters::assert_execution_configuration(&composition, source == "left");
        }
    }
}

#[test]
fn shared_role_parameters_keep_concrete_conflicts_and_user_json() {
    let first = "params = { shared = { 'a/b~c' = [{ input = 'value' }], known = 'first' } }";
    let second = "params = { shared = { 'a/b~c' = [{ input = 'value' }], known = 'second' } }";
    let (_dir, composition) = role_workspace(first, Some((second, "right")), "");
    role_readiness(&composition, WorkspaceReadiness::Invalid);

    // A literal sentinel-shaped object is a concrete value even beside unread data.
    let params = "params = { shared = { input = 'value' } }";
    let (_dir, mut composition) = role_workspace(params, Some((params, "right")), "");
    composition.runtimes[0].inputs = json!({"left":{"cwp://!cwr/unresolved-input":true},"right":{"env":RIGHT_ENV}});
    role_readiness(&composition, WorkspaceReadiness::Unsupported);
    composition.runtimes[0].inputs["right"] = json!({"cwp://!cwr/unresolved-input":true});
    role_readiness(&composition, WorkspaceReadiness::Ready);
    composition.runtimes[0].resolve_graph().unwrap();
    composition.runtimes[0].inputs["right"] = json!({"cwp://!cwr/unresolved-input":false});
    role_readiness(&composition, WorkspaceReadiness::Invalid);
    parameters::assert_execution_configuration(&composition, false);
}

#[test]
fn binding_parameters_preserve_unread_source_identity_against_roles() {
    let params = "params = { shared = [{ input = 'value' }] }";
    for source in ["left", "right"] {
        let binding = format!("params = {{ shared = [{{ input = '{source}' }}] }}");
        let (_dir, mut composition) = role_workspace(params, None, &binding);
        role_readiness(&composition, if source == "left" { WorkspaceReadiness::Ready } else { WorkspaceReadiness::Unsupported });
        composition.runtimes[0].inputs = json!({"left":"same", "right":"same"});
        composition.runtimes[0].resolve_graph().unwrap();
        composition.runtimes[0].inputs["right"] = json!("different");
        parameters::assert_execution_configuration(&composition, source == "left");
    }
}

#[test]
fn unread_role_entrypoints_defer_path_checks_and_concrete_entrypoints_validate() {
    for whole_params in [false, true] {
        let params = if whole_params { "params = { input = 'value' }" } else { "params = { entrypoint = { input = 'value' } }" };
        let (dir, mut composition) = role_workspace(params, None, "");
        role_readiness(&composition, if whole_params { WorkspaceReadiness::Invalid } else { WorkspaceReadiness::Ready });
        fs::write(dir.path().join("guide.md"), "role instructions").unwrap();
        for (entrypoint, valid) in [(json!("./guide.md"), true), (json!("./missing.md"), false), (json!(false), false)] {
            let value = if whole_params { json!({"entrypoint":entrypoint}) } else { entrypoint };
            composition.runtimes[0].inputs = json!({"left":value,"right":"unused"});
            role_readiness(&composition, if valid { WorkspaceReadiness::Ready } else { WorkspaceReadiness::Invalid });
            let graph = composition.runtimes[0].resolve_graph();
            if valid {
                let graph = graph.unwrap();
                assert_eq!(graph.actor_integrations[0].adapter_params["activated_entrypoints"], json!([dir.path().join("guide.md").canonicalize().unwrap()]));
            } else {
                parameters::assert_execution_configuration(&composition, false);
            }
        }
    }
}

#[test]
fn unread_entrypoints_remain_independent_across_roles() {
    let params = "params = { entrypoint = { input = 'value' } }";
    let (dir, mut composition) = role_workspace(params, Some((params, "right")), "");
    role_readiness(&composition, WorkspaceReadiness::Ready);
    for name in ["first.md", "second.md"] {
        fs::write(dir.path().join(name), "role instructions").unwrap();
    }
    composition.runtimes[0].inputs["left"] = json!("./first.md");
    role_readiness(&composition, WorkspaceReadiness::Ready);
    composition.runtimes[0].inputs["right"] = json!("./second.md");
    role_readiness(&composition, WorkspaceReadiness::Ready);
    let graph = composition.runtimes[0].resolve_graph().unwrap();
    assert_eq!(graph.actor_integrations[0].adapter_params["activated_entrypoints"].as_array().unwrap().len(), 2);
    composition.runtimes[0].inputs["right"] = json!(false);
    role_readiness(&composition, WorkspaceReadiness::Invalid);
    parameters::assert_execution_configuration(&composition, false);
}

#[test]
fn whole_role_parameters_reject_inputs_known_to_be_strings() {
    for type_ref in [requirements::SECRET_TYPE, requirements::PLAINTEXT_TYPE] {
        for environment in [true, false] {
            let (dir, mut composition) = role_workspace_with_type("params = { input = 'value' }", None, "", type_ref);
            // Role composition requires an object even without an adapter schema restriction.
            fs::write(dir.path().join("params.json"), "{}").unwrap();
            if !environment {
                let declarations = input("left", type_ref) + &input("right", type_ref)
                    + "[bindings]\nright = { value = 'unused' }\n";
                composition = workspace(dir.path(), &declarations, &(selected("ready")
                    + "[[runtimes.inputs]]\nref = '~@/left'\nas = 'left'\n[[runtimes.inputs]]\nref = '~@/right'\nas = 'right'\n"));
            }
            let impossible = environment || type_ref == requirements::SECRET_TYPE;
            role_readiness(&composition, if impossible { WorkspaceReadiness::Invalid } else { WorkspaceReadiness::NeedsSetup });
            if impossible {
                let report = composition.requirements_with_environment(&BTreeSet::from([LEFT_ENV.into(), RIGHT_ENV.into()])).unwrap();
                assert!(report.runtimes[0].issues.iter().any(|issue| issue.status == requirements::RequirementStatus::Invalid), "{report:?}");
            }
            composition = workspace(dir.path(), "", &(selected("ready")
                + "[runtimes.inputs]\nleft = 'private-role-value'\nright = 'unused'\n"));
            role_readiness(&composition, WorkspaceReadiness::Invalid);
            parameters::assert_execution_configuration(&composition, false);
            let warnings = composition.runtimes[0].resolve_graph().unwrap().warnings;
            assert!(!serde_json::to_string(&warnings).unwrap().contains("private-role-value"));
            if type_ref == requirements::PLAINTEXT_TYPE {
                composition.runtimes[0].inputs["left"] = json!({"mode":"plain-object"});
                role_readiness(&composition, WorkspaceReadiness::Ready);
                composition.runtimes[0].resolve_graph().unwrap();
            }
        }
    }
}

#[test]
fn public_configured_opener_validates_the_configured_actor_before_state() {
    for selected_actor in [Some("unknown"), Some("person"), None] {
        let dir = setup();
        fs::write(dir.path().join("other.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Other'\nslug = 'other'\n").unwrap();
        let path = dir.path().join("ready.toml");
        fs::write(&path, fs::read_to_string(&path).unwrap() + "[[imports]]\npath = './other.toml'\n").unwrap();
        let mut composition = workspace(dir.path(), "", &selected("ready"));
        composition.runtimes[0].acting_coworker = selected_actor.map(str::to_owned);
        let graph = composition.runtimes[0].resolve_graph().unwrap();
        let other = graph.actors.iter().find(|actor| actor.coworker_slug == "other").unwrap().actor_ref.clone();
        let result = LocalRuntime::open_configured_runtime(
            &composition.runtimes[0], LocalHostIdentity::new(&other, "cwp://example.test/cli"), LocalRuntimeOptions::default(),
        );
        if selected_actor == Some("unknown") {
            assert!(result.err().unwrap().to_string().contains("acting_coworker"));
            assert_eq!(composition.requirements().unwrap().readiness, WorkspaceReadiness::Invalid);
            assert!(composition.runtimes[0].catalog().is_err());
            assert!(!dir.path().join("state").exists());
        } else {
            // Caller identity remains effective, including when no actor is configured.
            let runtime = result.unwrap();
            runtime.catalog_as(&other).unwrap();
        }
    }
}

#[test]
fn direct_openers_require_root_volume_mappings_but_allow_optional_volumes() {
    for required in [false, true] {
        let dir = setup();
        let path = dir.path().join("ready.toml");
        fs::write(&path, fs::read_to_string(&path).unwrap() + &format!("[[volumes]]\nas = 'project'\naccess = 'read'\nrequired = {required}\n")).unwrap();
        let path = path.canonicalize().unwrap();
        let person = crate::v2026_09::graph::resolve_graph(dir.path().join("needs_secret.toml")).unwrap().actors[0].actor_ref.clone();
        let identity = LocalHostIdentity::new(person, "cwp://example.test/cli");
        let state = dir.path().join("direct.sqlite3");
        let result = LocalRuntime::open_with_state_path(&path, &state, identity.clone());
        if required {
            assert!(result.err().unwrap().to_string().contains("required volumes"));
            assert!(!state.exists());
            let error = LocalRuntime::open(&path, identity).err().unwrap().to_string();
            assert!(error.contains("required volumes"), "{error}");
            let paths = LocalWorkspacePaths::resolve(&path).unwrap();
            assert!(!paths.database_path().exists());
            // These directories belong to the fresh test workspace UUID.
            fs::remove_dir_all(paths.state_directory).unwrap();
            fs::remove_dir_all(paths.runtime_directory).unwrap();
        } else {
            result.unwrap();
        }
        let composition = workspace(dir.path(), "", &(selected("ready") + "[[runtimes.volumes]]\nref = '~@/files'\nas = 'project'\naccess = 'read'\n"));
        assert_eq!(composition.requirements().unwrap().readiness, WorkspaceReadiness::Ready);
        let runtime = composition.runtimes[0].open("cwp://example.test/cli").unwrap();
        runtime.reload_graph().unwrap();
    }
}

#[test]
fn mapping_errors_keep_all_loop_and_imported_requirements_visible() {
    for faulty in [
        "[[runtimes.inputs]]\nref = '~@/unknown'\nas = 'bad'\n",
        "[[runtimes.inputs]]\nref = '~@/team'\nas = 'bad'\n[[runtimes.inputs]]\nref = '~@/team'\nas = 'bad'\n",
    ] {
        let dir = setup();
        fs::write(dir.path().join("ready.toml"), format!(
            "type = 'cwp://org.coworkerprotocol/2026-09/schemas/loop'\n{}{}{}required = false\n[[imports]]\npath = './person.toml'\n[imports.inputs]\nleaf = {{ input = 'good' }}\n",
            input("bad", requirements::PLAINTEXT_TYPE), input("good", requirements::PLAINTEXT_TYPE), input("optional", requirements::SECRET_TYPE)
        )).unwrap();
        let person = dir.path().join("person.toml");
        fs::write(&person, fs::read_to_string(&person).unwrap() + &input("leaf", requirements::PLAINTEXT_TYPE)).unwrap();
        let composition = workspace(dir.path(), &(input("team", requirements::PLAINTEXT_TYPE) + "[bindings.team]\nvalue = 'private-mapped-value'\n"), &(selected("ready") + faulty + "[[runtimes.inputs]]\nref = '~@/team'\nas = 'good'\n"));
        let report = composition.requirements().unwrap();
        assert_eq!(report.readiness, WorkspaceReadiness::Invalid, "{report:?}");
        let rows = report.requirements.iter().filter(|row| row.runtime_name.as_deref() == Some("ready")).collect::<Vec<_>>();
        assert_eq!(rows.iter().map(|row| row.alias.as_str()).collect::<BTreeSet<_>>(), BTreeSet::from(["bad", "good", "optional", "leaf", "files"]));
        for alias in ["good", "leaf"] {
            let row = rows.iter().find(|row| row.alias == alias).unwrap();
            assert_eq!(row.status, RequirementStatus::Supplied);
            assert_eq!(row.sources[0].alias, "team");
        }
        assert!(!serde_json::to_string(&report).unwrap().contains("private-mapped-value"));
        assert!(composition.runtimes[0].open("cwp://example.test/cli").is_err());
        assert!(!dir.path().join("state").exists());
    }
}
