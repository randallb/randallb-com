use super::*;

#[test]
fn supplied_workspace_variables_require_approval_and_never_enter_retained_routes() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    for required in [true, false] {
        let root = fixture();
        let _env = EnvironmentGuard::isolate(root.path());
        let path = root.path().join("CWR.toml");
        fs::write(
            &path,
            fs::read_to_string(&path).unwrap().replace(
                "storage = 'workspace_variable'",
                &format!("storage = 'workspace_variable'\nrequired = {required}"),
            ),
        )
        .unwrap();
        let workspace = root.path().join("WORKSPACE.toml");
        fs::write(&workspace, "type = 'cwp://org.coworkerprotocol/2026-09/schemas/workspace'\n[[volumes]]\nname = 'project'\npath = '.'\n[[runtimes]]\nname = 'main'\npath = './CWR.toml'\nsource_roots = ['.']\nworking_directory = '.'\nvolume = 'project'\n[runtimes.inputs]\ncontext = 'unapproved-manifest-variable'\nresearch = 'supplied-secret'\n").unwrap();
        let report = inspect_project_connections(&workspace).unwrap();
        let variable = report
            .requirements
            .iter()
            .find(|r| r.alias == "context")
            .unwrap();
        assert!(!variable.configured && !variable.approved);
        assert!(
            report
                .requirements
                .iter()
                .find(|r| r.alias == "research")
                .unwrap()
                .configured
        );
        let compose = || {
            LocalWorkspaceComposition::load(&workspace)
                .unwrap()
                .runtimes[0]
                .resolve_graph()
                .unwrap()
        };
        let project = |graph: &ResolvedCwrGraph| {
            let binding = &graph.actor_integrations[0];
            (
                binding.process_adapter_route().unwrap(),
                binding.project_adapter_invocation(
                    INVOCATION,
                    "run--command",
                    json!({}),
                    json!({}),
                    json!({}),
                ),
            )
        };
        let graph = compose();
        assert!(!serde_json::to_string(&graph)
            .unwrap()
            .contains("unapproved-manifest-variable"));
        let (mut route, mut invocation) = project(&graph);
        assert!(!serde_json::to_string(&invocation)
            .unwrap()
            .contains("unapproved-manifest-variable"));
        let dependencies = &invocation.local_inputs.as_ref().unwrap().dependencies;
        assert_eq!(dependencies.len(), 1);
        assert_eq!(
            dependencies[0].selection,
            if required {
                InputSelection::Unavailable
            } else {
                InputSelection::Absent
            }
        );
        let prepared =
            handoff::prepare_with(&mut route, &mut invocation, &Default::default(), |_| {
                panic!("no Keychain dependency")
            });
        assert_eq!(prepared.is_err(), required);
        drop(prepared);
        if !required {
            assert!(invocation.integration_params["env"]
                .get("WORK_CONTEXT")
                .is_none());
            assert_eq!(
                invocation.integration_params["env"]["RESEARCH_KEY"],
                "supplied-secret"
            );
        }
        let store = FileStore(root.path().join("fake-keychain"));
        let saved = environment::save(
            root.path(),
            &store,
            "CONTEXT",
            EnvironmentEntryKind::Variable,
            VARIABLE,
            None,
            None,
        )
        .unwrap();
        save_project_connections(
            &workspace,
            &report.fingerprint,
            &report.grants_revision,
            BTreeMap::from([("context".into(), saved.entries[0].reference.clone())]),
        )
        .unwrap();
        let (mut route, mut invocation) = project(&compose());
        drop(
            handoff::prepare_with(&mut route, &mut invocation, &Default::default(), |_| {
                panic!("variable assignment must not read Keychain")
            })
            .unwrap(),
        );
        assert_eq!(
            invocation.integration_params["env"]["WORK_CONTEXT"],
            VARIABLE
        );
        assert_eq!(
            invocation.integration_params["env"]["RESEARCH_KEY"],
            "supplied-secret"
        );
    }
}
