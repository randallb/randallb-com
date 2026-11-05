use super::*;

fn partial(dir: &Path) -> ResolvedCwrGraph {
    resolve_graph_with_options(
        dir.join("CWR.toml"),
        &EmptyResourceResolver,
        &GraphLoadOptions {
            allow_partial: true,
            ..Default::default()
        },
    )
    .unwrap()
}

fn helper(dir: &Path, name: &str) {
    person(
        dir,
        name,
        &format!(
            "[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = '../{name}.toml'\nas = 'codex'\ndefault = true\n"
        ),
    );
    role(dir, name, "gpt-6-sol");
    integration(dir, name);
}

fn names(graph: &ResolvedCwrGraph) -> Vec<&str> {
    let mut names: Vec<_> = graph
        .actors
        .iter()
        .map(|actor| actor.coworker_slug.as_str())
        .collect();
    names.sort();
    names
}

#[test]
fn partial_loading_keeps_healthy_actions_with_legacy_and_missing_exports() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './good/COWORKER.toml'\n[[exports]]\npath = './legacy/COWORKER.toml'\n[[exports]]\npath = './ACTOR.toml'\n[[imports]]\npath = './missing.toml'\n",
    );
    helper(dir, "good");
    person(dir, "legacy", "ref = '~@/coworkers/legacy'\n");
    write(
        dir,
        "ACTOR.toml",
        &format!(
            "type = {ACTOR_TYPE:?}\nref = '~@/actors/legacy'\ncoworker_ref = '~@/coworkers/legacy'\n"
        ),
    );
    assert!(resolve_graph(dir.join("CWR.toml")).is_err());
    assert!(check(dir.join("CWR.toml")).is_err());
    let graph = partial(dir);
    assert_eq!(names(&graph), ["good"]);
    assert_eq!(graph.routes.len(), 1);
    assert_eq!(graph.actor_integrations.len(), 1);
    assert!(graph.routes[0].is_available());
    assert!(graph.warnings.len() >= 3);
    assert!(!graph
        .resources
        .iter()
        .any(|resource| resource.source.ends_with("ACTOR.toml")));
}

#[test]
fn required_role_and_adapter_errors_disable_only_their_coworkers() {
    for broken in ["missing_role", "invalid_schema", "invalid_adapter"] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        root(
            dir,
            "[[imports]]\npath = './good/COWORKER.toml'\n[[imports]]\npath = './bad/COWORKER.toml'\n",
        );
        helper(dir, "good");
        helper(dir, "bad");
        match broken {
            "missing_role" => fs::remove_file(dir.join("bad/ROLE.toml")).unwrap(),
            "invalid_schema" => write(dir, "bad/schema.json", "this is not JSON"),
            "invalid_adapter" => {
                let path = dir.join("bad/ROLE.toml");
                let text = fs::read_to_string(&path).unwrap();
                fs::write(
                    path,
                    text.replace(
                        "reasoning_effort = \"medium\"",
                        "reasoning_effort = \"secret-value-must-not-appear\"",
                    ),
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        let graph = partial(dir);
        assert_eq!(names(&graph), ["good"], "{broken}");
        assert_eq!(graph.routes.len(), 1, "{broken}");
        assert_eq!(graph.actor_integrations.len(), 1, "{broken}");
        assert!(!graph.warnings.is_empty(), "{broken}");
        assert!(!serde_json::to_string(&graph.warnings)
            .unwrap()
            .contains("secret-value-must-not-appear"));
        assert!(resolve_graph(dir.join("CWR.toml")).is_err());
    }
}

#[test]
fn broken_peer_import_and_default_do_not_disable_a_healthy_person() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './person/COWORKER.toml'\n");
    person(
        dir,
        "person",
        "[[imports]]\npath = '../bad/COWORKER.toml'\nas = 'bad'\ndefault_conversation_target = true\n",
    );
    write(dir, "bad/COWORKER.toml", "invalid TOML [[[private-value");
    let graph = partial(dir);
    assert_eq!(names(&graph), ["person"]);
    assert_eq!(graph.actors[0].default_conversation_target_ref, None);
    assert!(graph
        .warnings
        .iter()
        .any(|warning| warning.message.contains("default coworker")));
    assert!(!serde_json::to_string(&graph.warnings)
        .unwrap()
        .contains("private-value"));
}

#[test]
fn invalid_shared_role_disables_all_its_users_but_not_other_coworkers() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './good/COWORKER.toml'\n[[imports]]\npath = './one/COWORKER.toml'\n[[imports]]\npath = './two/COWORKER.toml'\n",
    );
    helper(dir, "good");
    for name in ["one", "two"] {
        person(dir, name, "[[roles]]\npath = '../shared/ROLE.toml'\n");
    }
    role(dir, "shared", "gpt-6-sol");
    write(dir, "shared/schema.json", "{}");
    fs::remove_file(dir.join("shared/schema.json")).unwrap();
    assert_eq!(names(&partial(dir)), ["good"]);
}

#[test]
fn partial_loading_still_rejects_invalid_root_configuration() {
    let temp = tempfile::tempdir().unwrap();
    write(
        temp.path(),
        "CWR.toml",
        &format!("type = {ROOT_TYPE:?}\nunknown_field = true\n"),
    );
    let result = resolve_graph_with_options(
        temp.path().join("CWR.toml"),
        &EmptyResourceResolver,
        &GraphLoadOptions {
            allow_partial: true,
            ..Default::default()
        },
    );
    assert!(result.is_err());
}

#[test]
fn repaired_coworker_returns_on_next_load() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './good/COWORKER.toml'\n[[imports]]\npath = './bad/COWORKER.toml'\n",
    );
    helper(dir, "good");
    helper(dir, "bad");
    let path = dir.join("bad/COWORKER.toml");
    let valid = fs::read_to_string(&path).unwrap();
    fs::write(&path, "invalid TOML").unwrap();
    assert_eq!(names(&partial(dir)), ["good"]);
    fs::write(path, valid).unwrap();
    let graph = partial(dir);
    assert_eq!(names(&graph), ["bad", "good"]);
    assert!(graph.warnings.is_empty());
}

#[test]
fn local_startup_settings_and_reload_keep_healthy_coworkers_available() {
    use crate::v2026_09::{
        host::inspect_local_workspace,
        local::{connections::inspect_project_connections, LocalHostIdentity, LocalRuntime},
    };
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './person/COWORKER.toml'\n[[imports]]\npath = './good/COWORKER.toml'\n[[imports]]\npath = './bad/COWORKER.toml'\n",
    );
    person(dir, "person", "");
    helper(dir, "good");
    person(dir, "bad", "ref = '~@/legacy'\n");
    let path = dir.join("CWR.toml");
    let workspace = inspect_local_workspace(&path).unwrap();
    assert_eq!(workspace.actors.len(), 2);
    assert!(!workspace.warnings.is_empty());
    let report = inspect_project_connections(&path).unwrap();
    assert!(report.requirements.is_empty());
    assert!(!report.warnings.is_empty());
    let actor = workspace
        .actors
        .iter()
        .find(|actor| actor.handle == "@person")
        .unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        &path,
        dir.join("state.sqlite3"),
        LocalHostIdentity::new(&actor.actor_ref, "cwp://example.test/desktop"),
    )
    .unwrap();
    assert!(runtime
        .catalog()
        .unwrap()
        .actions
        .iter()
        .any(|action| action.activation.starts_with("@good--")));
    assert!(!runtime
        .catalog()
        .unwrap()
        .known_actors
        .iter()
        .any(|actor| actor.handle == "@bad"));
    helper(dir, "bad");
    runtime.reload_graph().unwrap();
    assert!(runtime
        .catalog()
        .unwrap()
        .actions
        .iter()
        .any(|action| action.activation.starts_with("@bad--")));
    assert!(inspect_local_workspace(&path).unwrap().warnings.is_empty());
    fs::remove_file(dir.join("good/ROLE.toml")).unwrap();
    runtime.reload_graph().unwrap();
    let catalog = runtime.catalog().unwrap();
    assert!(!catalog
        .actions
        .iter()
        .any(|action| action.activation.starts_with("@good--")));
    assert!(catalog
        .actions
        .iter()
        .any(|action| action.activation.starts_with("@bad--")));
}

#[test]
fn unavailable_gateway_does_not_block_unrelated_coworker_actions() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './good/COWORKER.toml'\nas = 'good'\n[[imports]]\npath = './GATEWAY.toml'\nas = 'broken'\n",
    );
    helper(dir, "good");
    write(dir, "GATEWAY.toml", &format!("type = {GATEWAY_TYPE:?}\nrole = {{ ref = '~@/missing' }}\nintegration = {{ ref = '~@/missing' }}\naction_name = 'run--command'\n"));
    let graph = partial(dir);
    assert_eq!(names(&graph), ["good"]);
    assert_eq!(graph.routes.len(), 1);
    assert!(graph.gateways.is_empty());
    assert!(graph
        .warnings
        .iter()
        .any(|warning| warning.source.ends_with("GATEWAY.toml")));
    assert!(resolve_graph(dir.join("CWR.toml")).is_err());
}

#[test]
fn conflicting_shared_inputs_reject_activation_in_either_order() {
    for values in [["one", "two"], ["two", "one"]] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        root(
            dir,
            &format!(
                "[[imports]]\npath = './shared/COWORKER.toml'\nas = 'first'\n[imports.inputs]\nmode = '{}'\n[[imports]]\npath = './shared/COWORKER.toml'\nas = 'second'\n[imports.inputs]\nmode = '{}'\n[[imports]]\npath = './good/COWORKER.toml'\n",
                values[0], values[1]
            ),
        );
        person(
            dir,
            "shared",
            &format!(
                "[[inputs]]\nas = 'mode'\ntype = '{}'\n",
                requirements::PLAINTEXT_TYPE
            ),
        );
        helper(dir, "good");
        let error = resolve_graph_with_options(
            dir.join("CWR.toml"),
            &EmptyResourceResolver,
            &GraphLoadOptions {
                allow_partial: true,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("conflicting effective inputs"));
    }
}

#[test]
fn connection_inspection_does_not_disable_valid_unset_json_profiles() {
    use crate::v2026_09::local::connections::inspect_project_connections;
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        &format!(
            "[[inputs]]\nas = 'profile'\ntype = '{}'\n[[imports]]\npath = './helper/COWORKER.toml'\n[imports.inputs]\nprofile = {{ input = 'profile' }}\n[[imports]]\npath = './old/COWORKER.toml'\n",
            requirements::PLAINTEXT_TYPE
        ),
    );
    person(dir, "old", "ref = '~@/old'\n");
    person(
        dir,
        "helper",
        &format!(
            "[[inputs]]\nas = 'profile'\ntype = '{}'\n[[integrations]]\npath = '../profile.toml'\nas = 'profile'\n[integrations.inputs]\nprofile = {{ input = 'profile' }}\n",
            requirements::PLAINTEXT_TYPE
        ),
    );
    write(
        dir,
        "profile.toml",
        &format!(
            "type = {INTEGRATION_TYPE:?}\n[[inputs]]\nas = 'profile'\ntype = '{}'\n[params]\nprofile = {{ input = 'profile' }}\n[adapter]\npath = './adapter.toml'\n",
            requirements::PLAINTEXT_TYPE
        ),
    );
    write(
        dir,
        "adapter.toml",
        &format!(
            "type = {ADAPTER_TYPE:?}\nref = 'cwp://example.test/profile-adapter'\nname = 'Profile'\n[integration_params_schema]\npath = './profile.schema.json'\n"
        ),
    );
    write(
        dir,
        "profile.schema.json",
        r#"{"type":"object","required":["profile"],"properties":{"profile":{"type":"object"}}}"#,
    );
    let report = inspect_project_connections(dir.join("CWR.toml").as_path()).unwrap();
    assert_eq!(report.requirements.len(), 1);
    assert!(!report.requirements[0].configured);
    assert!(!report.warnings.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .all(|warning| warning.source.ends_with("old/COWORKER.toml")),
        "{:?}",
        report.warnings
    );
}

#[test]
fn broken_bindings_leave_working_bindings_and_rank_order_intact() {
    for failure in ["missing", "invalid_adapter", "invalid_params"] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        root(dir, "[[imports]]\npath = './person/COWORKER.toml'\n[[imports]]\npath = './viewer/COWORKER.toml'\n");
        person(dir, "viewer", "");
        role(dir, "person", "gpt-6-sol");
        for name in ["bad", "host", "other"] {
            integration(dir, name);
        }
        match failure {
            "missing" => fs::remove_file(dir.join("bad.toml")).unwrap(),
            "invalid_adapter" => write(
                dir,
                "bad.toml",
                &format!(
                    "type = {INTEGRATION_TYPE:?}\n[adapter]\nref = 'cwp://example/unsupported-adapter'\n"
                ),
            ),
            "invalid_params" => write(
                dir,
                "bad.toml",
                &format!(
                    "type = {INTEGRATION_TYPE:?}\nparams = 42\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/codex'\n"
                ),
            ),
            _ => unreachable!(),
        }
        person(
            dir,
            "person",
            "message_binding_preference = ['bad', 'other', 'host']\n[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = '../bad.toml'\nas = 'bad'\ndefault = true\n[[integrations]]\npath = '../host.toml'\nas = 'host'\n[[integrations]]\npath = '../other.toml'\nas = 'other'\n",
        );
        let graph = partial(dir);
        assert_eq!(names(&graph), ["person", "viewer"], "{failure}");
        assert_eq!(graph.actor_integrations.len(), 2, "{failure}");
        assert_eq!(graph.routes.len(), 2, "{failure}");
        let actor = graph
            .actors
            .iter()
            .find(|actor| actor.coworker_slug == "person")
            .unwrap();
        assert!(actor.default_actor_integration_ref.is_none(), "{failure}");
        assert_eq!(
            actor.message_binding_preference,
            [
                format!("{}/bindings/other", actor.actor_ref),
                format!("{}/bindings/host", actor.actor_ref),
            ]
        );
        assert!(graph
            .actor_integrations
            .iter()
            .all(|binding| binding.is_available()));
        assert!(graph
            .warnings
            .iter()
            .any(|warning| warning.message.contains("Binding bad")));
        assert!(check(dir.join("CWR.toml")).is_err());
        use crate::v2026_09::local::{LocalHostIdentity, LocalRuntime};
        let viewer = graph
            .actors
            .iter()
            .find(|actor| actor.coworker_slug == "viewer")
            .unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            dir.join("CWR.toml"),
            dir.join("state.sqlite3"),
            LocalHostIdentity::new(&viewer.actor_ref, "cwp://example.test/desktop"),
        )
        .unwrap();
        let catalog = runtime.catalog().unwrap();
        assert!(catalog
            .known_actors
            .iter()
            .any(|actor| actor.handle == "@person"));
        assert!(!catalog.actors.iter().any(|actor| actor.handle == "@person"));
        assert!(!catalog
            .actions
            .iter()
            .any(|action| action.activation.starts_with("@person--")));
        integration(dir, "bad");
        let repaired = partial(dir);
        assert_eq!(repaired.actor_integrations.len(), 3);
        assert!(repaired.warnings.is_empty());
    }
}

#[test]
fn invalid_codex_role_settings_do_not_disable_claude_for_the_same_coworker() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './person/COWORKER.toml'\n");
    role(dir, "person", "gpt-6-sol");
    let path = dir.join("person/ROLE.toml");
    let text = fs::read_to_string(&path).unwrap().replace(
        "reasoning_effort = \"medium\"",
        "reasoning_effort = \"invalid-secret-setting\"",
    );
    fs::write(&path, format!("{text}\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/claude'\n[adapters.params]\nentrypoint = './PROMPT.md'\nmodel = 'test-claude'\n")).unwrap();
    integration(dir, "codex");
    write(
        dir,
        "claude.toml",
        &format!(
            "type = {INTEGRATION_TYPE:?}\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/claude'\n"
        ),
    );
    person(
        dir,
        "person",
        "message_binding_preference = ['codex', 'claude']\n[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = '../codex.toml'\nas = 'codex'\ndefault = true\n[[integrations]]\npath = '../claude.toml'\nas = 'claude'\n",
    );
    let graph = partial(dir);
    assert_eq!(names(&graph), ["person"]);
    assert_eq!(graph.actor_integrations.len(), 1);
    assert!(graph.actor_integrations[0].adapter_ref.ends_with("/claude"));
    assert_eq!(graph.routes.len(), 1);
    assert!(!serde_json::to_string(&graph.warnings)
        .unwrap()
        .contains("invalid-secret-setting"));
    assert!(check(dir.join("CWR.toml")).is_err());
}

#[test]
fn disabled_native_binding_does_not_block_the_callers_working_tools() {
    for selected in ["", "binding = 'bad'\n"] {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        root(dir, "[[imports]]\npath = './caller/COWORKER.toml'\n");
        helper(dir, "caller");
        helper(dir, "target");
        integration(dir, "host");
        person(dir, "target", "[[roles]]\npath = './ROLE.toml'\n[[integrations]]\npath = '../missing.toml'\nas = 'bad'\ndefault = true\n[[integrations]]\npath = '../host.toml'\nas = 'host'\n");
        let caller = dir.join("caller/COWORKER.toml");
        let text = fs::read_to_string(&caller).unwrap();
        fs::write(&caller, format!("{text}\n[[imports]]\npath = '../target/COWORKER.toml'\nas = 'target'\n[[native_tools]]\ncoworker = '~@/target'\naction = 'make--target'\n{selected}[[native_tools]]\ncoworker = '~@/target'\naction = 'make--target'\nbinding = 'host'\n")).unwrap();
        let mut graph = partial(dir);
        assert_eq!(names(&graph), ["caller", "target"]);
        let caller = graph
            .actors
            .iter()
            .find(|actor| actor.coworker_slug == "caller")
            .unwrap();
        assert_eq!(caller.native_tools.len(), 1);
        assert!(caller.native_tools[0]
            .binding_ref
            .as_deref()
            .unwrap()
            .ends_with("/host"));
        let caller_ref = caller.actor_ref.clone();
        crate::v2026_09::execution::retain_native_tool_definitions(&mut graph).unwrap();
        let binding = graph
            .actor_integrations
            .iter()
            .find(|binding| binding.actor_ref == caller_ref)
            .unwrap();
        assert_eq!(
            binding.adapter_params["cwr_native_tools"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(graph
            .warnings
            .iter()
            .any(|warning| warning.message.contains("Native tool disabled")));
    }
}

#[path = "partial_namespace.rs"]
mod namespace;

#[path = "partial_state.rs"]
mod state;
