use super::*;
use serde_json::json;

#[path = "actor_gateway_tests.rs"]
mod actor_gateway_tests;
#[path = "decisions.rs"]
mod decisions;
#[path = "partial.rs"]
mod partial;

#[path = "resolution.rs"]
mod resolution;

#[path = "conversation_target.rs"]
mod conversation_target;

#[path = "volume_ownership.rs"]
mod volume_ownership;

fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn root(root: &Path, imports: &str) {
    write(
        root,
        "CWR.toml",
        &format!("type = {ROOT_TYPE:?}\n{imports}"),
    );
}
fn person(root: &Path, name: &str, extra: &str) {
    write(
        root,
        &format!("{name}/COWORKER.toml"),
        &format!("type = {COWORKER_TYPE:?}\nname = {name:?}\nslug = {name:?}\n{extra}"),
    );
}
fn role(root: &Path, name: &str, model: &str) {
    write(
        root,
        &format!("{name}/ROLE.toml"),
        &format!(
            r#"type = "{ROLE_TYPE}"
[[adapters]]
ref = "cwp://org.coworkerprotocol/2026-09/adapters/codex"
[adapters.params]
entrypoint = "./PROMPT.md"
model = "{model}"
reasoning_effort = "medium"
[[actions]]
verb = "make"
object = "{name}"
title = "Make"
description = "Make something"
[actions.input_schema]
path = "./schema.json"
[actions.output_schema]
path = "./schema.json"
"#
        ),
    );
    write(root, &format!("{name}/PROMPT.md"), name);
    write(root, &format!("{name}/schema.json"), r#"{"type":"object"}"#);
}
fn integration(root: &Path, name: &str) {
    write(
        root,
        &format!("{name}.toml"),
        &format!(
            "type = {INTEGRATION_TYPE:?}\n[adapter]\nref = \"cwp://org.coworkerprotocol/2026-09/adapters/codex\"\n"
        ),
    );
}

#[test]
fn legacy_slack_declarations_still_load_without_a_host() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(
        dir,
        "[[imports]]\npath = './person/COWORKER.toml'\nas = 'person'\n",
    );
    person(
        dir,
        "person",
        "[[integrations]]\npath = '../slack.toml'\nas = 'slack'\ndefault = true\n[integrations.params.identity]\nkind = 'native_user'\nuser_id = 'U123'\n",
    );
    write(
        dir,
        "slack.toml",
        &format!(
            "type = {INTEGRATION_TYPE:?}\n[params]\nteam_id = 'T123'\n[adapter]\nref = {:?}\n",
            crate::adapter::SLACK_ADAPTER_REF
        ),
    );

    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    assert_eq!(graph.actor_integrations.len(), 1);
    assert_eq!(
        graph.actor_integrations[0].adapter_ref,
        crate::adapter::SLACK_ADAPTER_REF
    );
}

#[test]
fn transitive_cycles_and_aliases_converge_on_file_identity() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    person(
        dir,
        "carl",
        "[[imports]]\npath = '../bob/COWORKER.toml'\nas = 'bob'\n[[imports]]\npath = '../shelly/COWORKER.toml'\nas = 'shell'\n",
    );
    person(
        dir,
        "bob",
        "[[imports]]\npath = '../alice/COWORKER.toml'\nas = 'alice'\n",
    );
    person(
        dir,
        "alice",
        "[[imports]]\npath = '../shelly/COWORKER.toml'\nas = 'shelly'\n[[imports]]\npath = '../carl/COWORKER.toml'\nas = 'carl'\n",
    );
    person(dir, "shelly", "");
    root(
        dir,
        "[[imports]]\npath = './carl/COWORKER.toml'\nas = 'carl'\n",
    );
    let first = resolve_graph(dir.join("CWR.toml")).unwrap();
    assert_eq!(first.actors.len(), 4);
    root(
        dir,
        "[[imports]]\npath = './shelly/../carl/COWORKER.toml'\nas = 'different'\n[[imports]]\npath = './shelly/COWORKER.toml'\n",
    );
    let second = resolve_graph(dir.join("CWR.toml")).unwrap();
    assert_eq!(first.reference, second.reference);
    assert_eq!(first.actors, second.actors);
    assert!(first
        .actors
        .iter()
        .all(|actor| actor.actor_ref == actor.coworker_ref));
}

#[test]
fn roles_compose_in_order_and_snapshot_sources_at_activation() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './bob/COWORKER.toml'\n");
    role(dir, "first", "gpt-5");
    role(dir, "second", "gpt-5");
    integration(dir, "codex");
    person(
        dir,
        "bob",
        "[[roles]]\npath = '../first/ROLE.toml'\n[[roles]]\npath = '../second/ROLE.toml'\n[[integrations]]\npath = '../codex.toml'\nas = 'codex'\n",
    );
    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    assert_eq!(graph.routes.len(), 2);
    assert_eq!(graph.actors[0].role_refs.len(), 2);
    assert_eq!(
        graph.actors[0].provider_model.as_deref(),
        Some("Codex gpt-5")
    );
    assert_eq!(
        graph.actors[0].default_actor_integration_ref.as_ref(),
        Some(&graph.actor_integrations[0].actor_integration_ref)
    );
    let params = &graph.actor_integrations[0].adapter_params;
    let paths = params["activated_entrypoints"].as_array().unwrap();
    assert_eq!(
        params["activated_sources"][paths[0].as_str().unwrap()],
        "first"
    );
    assert_eq!(
        params["activated_sources"][paths[1].as_str().unwrap()],
        "second"
    );
    write(dir, "first/PROMPT.md", "edited");
    assert_eq!(
        params["activated_sources"][paths[0].as_str().unwrap()],
        "first"
    );
    assert_ne!(graph.routes[0].role_ref, graph.routes[1].role_ref);
    assert!(graph
        .resources
        .iter()
        .all(|resource| resource.type_ref != ACTOR_TYPE));
}

#[test]
fn role_collisions_and_conflicting_shared_models_reject() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './bob/COWORKER.toml'\n");
    role(dir, "first", "gpt-5");
    role(dir, "second", "gpt-4");
    integration(dir, "codex");
    person(
        dir,
        "bob",
        "[[roles]]\npath = '../first/ROLE.toml'\n[[roles]]\npath = '../second/ROLE.toml'\n[[integrations]]\npath = '../codex.toml'\nas = 'codex'\n",
    );
    assert!(resolve_graph(dir.join("CWR.toml"))
        .unwrap_err()
        .to_string()
        .contains("conflicting shared parameter model"));
    let path = dir.join("second/ROLE.toml");
    let contents = fs::read_to_string(&path)
        .unwrap()
        .replace("object = \"second\"", "object = \"first\"");
    fs::write(path, contents).unwrap();
    assert!(resolve_graph(dir.join("CWR.toml"))
        .unwrap_err()
        .to_string()
        .contains("overlapping action make--first"));
}

#[test]
fn input_forwarding_validates_and_conflicts_without_disclosing_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    write(
        dir,
        "schema.json",
        r#"{"type":"object","required":["token"],"properties":{"token":{"type":"string","minLength":5,"x-cwp-secret":true}},"additionalProperties":false}"#,
    );
    person(dir, "shelly", "[input_schema]\npath = '../schema.json'\n");
    root(
        dir,
        "[input_schema]\npath = './schema.json'\n[[imports]]\npath = './shelly/COWORKER.toml'\nas = 'one'\n[imports.inputs]\ntoken = { input = 'token' }\n[[imports]]\npath = './shelly/COWORKER.toml'\nas = 'two'\n[imports.inputs]\ntoken = { input = 'token' }\n",
    );
    let options = GraphLoadOptions {
        inputs: json!({"token":"private-value"}),
        ..GraphLoadOptions::default()
    };
    let graph =
        resolve_graph_with_options(dir.join("CWR.toml"), &EmptyResourceResolver, &options).unwrap();
    assert_eq!(graph.actors.len(), 1);
    let manifest = dir.join("CWR.toml");
    let contents = fs::read_to_string(&manifest).unwrap().replacen(
        "token = { input = 'token' }",
        "token = 'different-private-value'",
        1,
    );
    fs::write(&manifest, contents).unwrap();
    let error = resolve_graph_with_options(&manifest, &EmptyResourceResolver, &options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("conflicting effective inputs"));
    assert!(!error.contains("private-value"));
    let invalid = GraphLoadOptions {
        inputs: json!({"token":"abc"}),
        ..GraphLoadOptions::default()
    };
    let error = resolve_graph_with_options(&manifest, &EmptyResourceResolver, &invalid)
        .unwrap_err()
        .to_string();
    assert!(!error.contains("abc"));
    assert_eq!(
        inspect_input_schema(&manifest, &EmptyResourceResolver)
            .unwrap()
            .unwrap()["required"],
        json!(["token"])
    );
}

#[test]
fn canonical_source_roots_reject_path_and_ref_symlink_escapes() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let dir = temp.path();
    person(outside.path(), "outside", "");
    std::os::unix::fs::symlink(outside.path(), dir.join("escape")).unwrap();
    root(
        dir,
        "[[imports]]\npath = './escape/outside/COWORKER.toml'\n",
    );
    assert!(resolve_graph(dir.join("CWR.toml"))
        .unwrap_err()
        .to_string()
        .contains("source roots"));
    struct Host(PathBuf);
    impl CheckResourceResolver for Host {
        fn resolve(&self, _: &str) -> CwrResult<Option<PathBuf>> {
            Ok(Some(self.0.clone()))
        }
    }
    root(dir, "[[imports]]\nref = 'cwp://com.example/outside'\n");
    let host = Host(outside.path().join("outside/COWORKER.toml"));
    assert!(resolve_graph_with_resolver(dir.join("CWR.toml"), &host)
        .unwrap_err()
        .to_string()
        .contains("source roots"));
}

#[test]
fn known_ref_and_path_share_identity_and_exports_keep_owned_namespaces() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    person(dir, "shelly", "");
    write(
        dir,
        "team/CWR.toml",
        &format!(
            "type = {ROOT_TYPE:?}\n[[exports]]\npath = '../shelly/COWORKER.toml'\nas = 'shell'\n"
        ),
    );
    struct Host(PathBuf);
    impl CheckResourceResolver for Host {
        fn resolve(&self, reference: &str) -> CwrResult<Option<PathBuf>> {
            Ok((reference == "cwp://com.example/shelly").then(|| self.0.clone()))
        }
    }
    let host = Host(dir.join("shelly/COWORKER.toml"));
    for imports in [
        "[[imports]]\nref = 'cwp://com.example/shelly'\nas = 'one'\n[[imports]]\npath = './shelly/COWORKER.toml'\nas = 'two'\n",
        "[[imports]]\npath = './shelly/COWORKER.toml'\nas = 'two'\n[[imports]]\nref = 'cwp://com.example/shelly'\nas = 'one'\n",
        "[[imports]]\npath = './team/CWR.toml'\nas = 'team'\n[[imports]]\nref = '~@/team/shell'\nas = 'shell'\n",
    ] {
        root(dir, imports);
        let graph = resolve_graph_with_resolver(dir.join("CWR.toml"), &host).unwrap();
        assert_eq!(graph.actors.len(), 1);
        assert_eq!(
            graph.actors[0].actor_ref,
            resolver::canonical_source_ref(
                &dir.join("shelly/COWORKER.toml").canonicalize().unwrap()
            )
        );
    }
}

#[test]
fn named_bindings_require_one_default_and_every_role_supports_each_binding() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    root(dir, "[[imports]]\npath = './bob/COWORKER.toml'\n");
    role(dir, "first", "gpt-5");
    integration(dir, "one");
    integration(dir, "two");
    let declarations = "[[roles]]\npath = '../first/ROLE.toml'\n[[integrations]]\npath = '../one.toml'\nas = 'one'\n[[integrations]]\npath = '../two.toml'\nas = 'two'\n";
    person(dir, "bob", declarations);
    assert!(resolve_graph(dir.join("CWR.toml"))
        .unwrap_err()
        .to_string()
        .contains("exactly one default"));
    person(
        dir,
        "bob",
        &declarations.replace("as = 'one'", "as = 'one'\ndefault = true"),
    );
    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    assert_eq!(graph.actor_integrations.len(), 2);
    assert!(graph.actors[0]
        .default_actor_integration_ref
        .as_ref()
        .unwrap()
        .ends_with("/bindings/one"));
    write(
        dir,
        "two.toml",
        &format!(
            "type = {INTEGRATION_TYPE:?}\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/claude'\n"
        ),
    );
    assert!(resolve_graph(dir.join("CWR.toml"))
        .unwrap_err()
        .to_string()
        .contains("does not support selected role"));
}

#[test]
fn standalone_role_creates_no_coworker_or_invocation_and_instruction_sources_are_frozen() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    role(dir, "first", "gpt-5");
    root(dir, "[[imports]]\npath = './first/ROLE.toml'\n");
    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    assert!(graph.actors.is_empty());
    assert!(graph.routes.is_empty());
    let source = dir.join("first/ROLE.toml");
    let role = fs::read_to_string(&source).unwrap();
    fs::write(
        &source,
        role.replace(
            "[[adapters]]",
            "[[instructions]]\npath = '../shared.md'\n[[adapters]]",
        ),
    )
    .unwrap();
    write(dir, "shared.md", "shared activated instructions");
    integration(dir, "codex");
    person(
        dir,
        "bob",
        "[[roles]]\npath = '../first/ROLE.toml'\n[[integrations]]\npath = '../codex.toml'\nas = 'codex'\n",
    );
    root(dir, "[[imports]]\npath = './bob/COWORKER.toml'\n");
    let graph = resolve_graph(dir.join("CWR.toml")).unwrap();
    let params = &graph.actor_integrations[0].adapter_params;
    let first = params["activated_entrypoints"][0].as_str().unwrap();
    fs::remove_file(dir.join("shared.md")).unwrap();
    assert_eq!(
        params["activated_sources"][first],
        "shared activated instructions"
    );
    assert!(resolve_graph(dir.join("CWR.toml")).is_err());
}
