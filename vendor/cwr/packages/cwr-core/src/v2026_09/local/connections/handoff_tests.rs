use super::*;
use crate::{adapter::*, credentials::SecretStore, workspace_environment as environment};
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const SECRET: &str = "private-secret-sentinel-082735";
const VARIABLE: &str = "private-variable-sentinel-192745";
const INVOCATION: &str = "cwp://example.test/invocations/input-test";
struct FileStore(PathBuf);
impl SecretStore for FileStore {
    fn put(&self, id: &str, value: &str) -> Result<(), &'static str> {
        fs::create_dir_all(&self.0).unwrap();
        fs::write(self.0.join(id), value).map_err(|_| "write")
    }
    fn get(&self, id: &str) -> Result<Option<String>, &'static str> {
        Ok(fs::read_to_string(self.0.join(id)).ok())
    }
    fn contains(&self, id: &str) -> Result<bool, &'static str> {
        Ok(self.0.join(id).exists())
    }
    fn remove(&self, id: &str) -> Result<(), &'static str> {
        let _ = fs::remove_file(self.0.join(id));
        Ok(())
    }
}

// Exercise production grant validation with this fixture's disposable store.
fn save_project_connections(
    path: &Path,
    fingerprint: &str,
    revision: &str,
    assignments: BTreeMap<String, String>,
) -> CwrResult<ProjectConnections> {
    let store = FileStore(path.parent().unwrap().join("fake-keychain"));
    mutation::save_with(path, fingerprint, revision, assignments, |id| {
        Ok(store.contains(id)?)
    })
}

pub(super) struct EnvironmentGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl EnvironmentGuard {
    pub(super) fn isolate(root: &Path) -> Self {
        let vars = [
            "WORKLOOP_STATE_HOME",
            "CWR_TEST_LEGACY_STATE_HOME",
            "WORKLOOP_STORAGE_IDENTITY",
        ];
        let previous = Self(
            vars.into_iter()
                .map(|name| (name, std::env::var_os(name)))
                .collect(),
        );
        std::env::set_var(vars[0], root.join("state"));
        std::env::set_var(vars[1], root.join("legacy"));
        std::env::set_var(vars[2], "input-test");
        previous
    }
}
impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }
}

fn fixture() -> tempfile::TempDir {
    let root = super::tests::fixture();
    for (file, alias) in [
        ("CWR.toml", "context"),
        ("shell.toml", "context"),
        ("integration.toml", "context"),
    ] {
        let path = root.path().join(file);
        let text = fs::read_to_string(&path).unwrap();
        let declaration = format!("[[inputs]]\nas = '{alias}'\ntype = '{PLAINTEXT_TYPE}'\nstorage = 'workspace_variable'\n");
        fs::write(
            path,
            text.replacen("[[inputs]]", &(declaration + "[[inputs]]"), 1),
        )
        .unwrap();
    }
    let path = root.path().join("CWR.toml");
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap()
            + "[[imports.inputs]]\nref = '~@/context'\nas = 'context'\n",
    )
    .unwrap();
    let path = root.path().join("shell.toml");
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap()
            + "[[integrations.inputs]]\nref = '~@/context'\nas = 'context'\n",
    )
    .unwrap();
    let path = root.path().join("integration.toml");
    fs::write(
        &path,
        fs::read_to_string(&path).unwrap() + "WORK_CONTEXT = { input = 'context' }\n",
    )
    .unwrap();
    root
}

fn setup(root: &Path) -> FileStore {
    let store = FileStore(root.join("fake-keychain"));
    environment::save(
        root,
        &store,
        "TOKEN",
        EnvironmentEntryKind::Secret,
        SECRET,
        None,
        None,
    )
    .unwrap();
    let entries = environment::save(
        root,
        &store,
        "CONTEXT",
        EnvironmentEntryKind::Variable,
        VARIABLE,
        None,
        None,
    )
    .unwrap()
    .entries;
    let assignments = entries
        .into_iter()
        .map(|e| {
            (
                (if e.kind == EnvironmentEntryKind::Secret {
                    "research"
                } else {
                    "context"
                })
                .into(),
                e.reference,
            )
        })
        .collect();
    let path = root.join("CWR.toml");
    let report = inspect_project_connections(&path).unwrap();
    save_project_connections(
        &path,
        &report.fingerprint,
        &report.grants_revision,
        assignments,
    )
    .unwrap();
    store
}

fn projected(root: &Path) -> (ResolvedProcessAdapterRoute, AdapterInvocation) {
    let graph = resolve_local_graph(&root.join("CWR.toml")).unwrap();
    let binding = &graph.actor_integrations[0];
    let invocation = binding.project_adapter_invocation(
        INVOCATION,
        "run--command",
        json!({}),
        json!({}),
        json!({}),
    );
    let retained = serde_json::to_string(&invocation).unwrap();
    for sentinel in [VARIABLE, SECRET] {
        assert!(!retained.contains(sentinel));
        assert!(!serde_json::to_string(&graph).unwrap().contains(sentinel));
    }
    assert_eq!(
        invocation.local_inputs.as_ref().unwrap().dependencies.len(),
        2
    );
    (
        binding.process_adapter_route().unwrap(),
        serde_json::from_str(&retained).unwrap(),
    )
}

#[test]
fn gateway_only_binding_keeps_private_input_plan_and_rejects_removal() {
    let _lock = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    fs::write(root.path().join("role.toml"), fs::read_to_string(root.path().join("role.toml")).unwrap()
        + "[[actions]]\nverb = 'run'\nobject = 'command'\ntitle = 'Run command'\ndescription = 'Run command'\n[actions.input_schema]\npath = './input.json'\n[actions.output_schema]\npath = './output.json'\n").unwrap();
    fs::write(root.path().join("input.json"), r#"{"type":"object"}"#).unwrap();
    fs::write(root.path().join("output.json"), r#"{"type":"object"}"#).unwrap();
    let _environment = EnvironmentGuard::isolate(root.path());
    let _store = setup(root.path());
    let mut graph = resolve_local_graph(&root.path().join("CWR.toml")).unwrap();
    let binding = graph.actor_integrations[0].clone();
    let route = graph
        .routes
        .iter()
        .find(|route| route.actor_integration_ref == binding.actor_integration_ref)
        .unwrap()
        .clone();
    let mut ingress = route.clone();
    ingress.action_name = "receive--event".into();
    let mut delivery = route.clone();
    delivery.action_name = "deliver--message".into();
    graph
        .gateways
        .push(crate::v2026_09::graph::ResolvedGateway {
            resource_ref: "cwp://example.test/gateways/managed-input".into(),
            request_refs: vec!["cwp://example.test/gateways/managed-input".into()],
            route: route.clone(),
            ingress_route: Some(ingress),
            delivery_route: Some(delivery),
            connection_ref: Some("cwp://example.test/connections/managed-input".into()),
        });
    graph.actor_integrations.clear();
    let selected = handoff::approved_binding(&graph, &binding.actor_integration_ref).unwrap();
    assert_eq!(selected.input_plan.dependencies.len(), 2);
    assert_eq!(
        capture_for_binding(&selected.input_plan, &selected.input_uses).unwrap(),
        capture_for_binding(&binding.input_plan, &binding.input_uses).unwrap()
    );
    for secret in [SECRET, VARIABLE] {
        assert!(!serde_json::to_string(&graph).unwrap().contains(secret));
    }
    graph.gateways[0]
        .ingress_route
        .as_mut()
        .unwrap()
        .input_plan
        .dependencies
        .clear();
    assert!(handoff::approved_binding(&graph, &binding.actor_integration_ref).is_err());
    graph.gateways.clear();
    assert!(handoff::approved_binding(&graph, &binding.actor_integration_ref).is_err());
}

#[test]
fn declared_multi_entry_gateway_approves_one_shared_binding() {
    let _lock = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    let _environment = EnvironmentGuard::isolate(root.path());
    fs::write(root.path().join("input.json"), r#"{"type":"object"}"#).unwrap();
    fs::write(root.path().join("output.json"), r#"{"type":"object"}"#).unwrap();
    let actions = ["request", "ingress", "delivery"]
        .into_iter()
        .map(|name| format!("[[actions]]\nverb = 'handle'\nobject = '{name}'\ntitle = 'Handle {name}'\ndescription = 'Handle {name}'\n[actions.input_schema]\npath = './input.json'\n[actions.output_schema]\npath = './output.json'\n"))
        .collect::<String>();
    fs::write(
        root.path().join("role.toml"),
        fs::read_to_string(root.path().join("role.toml")).unwrap() + &actions,
    )
    .unwrap();
    fs::write(root.path().join("gateway-integration.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n").unwrap();
    fs::write(root.path().join("gateway.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/gateway'\nrole = { ref = '~@/behavior' }\nintegration = { ref = '~@/execution' }\naction_name = 'handle--request'\ningress_action_name = 'handle--ingress'\ndelivery_action_name = 'handle--delivery'\nconnection_ref = 'cwp://example.test/connections/multi'\n[[imports]]\npath = './role.toml'\nas = 'behavior'\n[[imports]]\npath = './gateway-integration.toml'\nas = 'execution'\n").unwrap();
    let manifest = root.path().join("CWR.toml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest).unwrap()
            + "[[imports]]\npath = './gateway.toml'\nas = 'gateway'\n",
    )
    .unwrap();
    let _store = setup(root.path());

    let graph = resolve_local_graph(&manifest).unwrap();
    let gateway = &graph.gateways[0];
    let binding_ref = &gateway.route.actor_integration_ref;
    assert_eq!(
        gateway
            .ingress_route
            .as_ref()
            .unwrap()
            .actor_integration_ref,
        *binding_ref
    );
    assert_eq!(
        gateway
            .delivery_route
            .as_ref()
            .unwrap()
            .actor_integration_ref,
        *binding_ref
    );
    let selected = handoff::approved_binding(&graph, binding_ref).unwrap();
    assert_eq!(selected.integration_ref, gateway.route.integration_ref);
    let mut mismatched = graph.clone();
    mismatched.gateways[0]
        .ingress_route
        .as_mut()
        .unwrap()
        .integration_params = json!({"changed": true});
    assert!(handoff::approved_binding(&mismatched, binding_ref).is_err());
}

fn mutate_child(root: &Path, mode: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "v2026_09::local::connections::handoff_tests::mutation_child",
            "--nocapture",
        ])
        .env("CWR_INPUT_TEST_CHILD_ROOT", root)
        .env("CWR_INPUT_TEST_CHILD_MODE", mode)
        .stdout(Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn mutation_child() {
    let Some(root) = std::env::var_os("CWR_INPUT_TEST_CHILD_ROOT").map(PathBuf::from) else {
        return;
    };
    fs::write(root.join("mutation-started"), "").unwrap();
    let path = root.join("CWR.toml");
    if std::env::var("CWR_INPUT_TEST_CHILD_MODE").unwrap() == "revoke" {
        let target = connection_target(&path).unwrap();
        let definition = definition_for_target(&target).unwrap();
        let revision = grants_revision(&read_grants(&target.grants_path).unwrap()).unwrap();
        save_project_connections(&path, &definition.fingerprint, &revision, BTreeMap::new())
            .unwrap();
    } else {
        let entry = environment::entries(&root)
            .unwrap()
            .into_iter()
            .find(|e| e.kind == EnvironmentEntryKind::Variable)
            .unwrap();
        environment::save(
            &root,
            &FileStore(root.join("fake-keychain")),
            "CONTEXT",
            EnvironmentEntryKind::Variable,
            VARIABLE,
            Some(&entry.reference),
            Some(entry.revision),
        )
        .unwrap();
    }
    fs::write(root.join("mutation-committed"), "").unwrap();
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn cross_process_mutation_before_handoff_rejects_complete_mixed_input_attempt() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    for mode in ["revoke", "replace"] {
        let root = fixture();
        let _env = EnvironmentGuard::isolate(root.path());
        let store = setup(root.path());
        let (mut route, mut invocation) = projected(root.path());
        let retained = invocation.clone();
        let result =
            handoff::prepare_with(&mut route, &mut invocation, &Default::default(), |id| {
                // Native access is outside the critical section: another process can
                // finish revocation or same-value replacement before final validation.
                assert!(mutate_child(root.path(), mode).wait().unwrap().success());
                store.get(id)?.ok_or_else(|| "missing".into())
            });
        assert!(result.is_err());
        assert_eq!(invocation, retained);
        assert!(root.path().join("mutation-committed").exists());
    }
}

#[test]
fn cross_process_handoff_finishes_before_revocation_and_releases_before_response() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    let store = setup(root.path());
    let (mut route, mut invocation) = projected(root.path());
    let retained = invocation.clone();
    let guard = handoff::prepare_with(&mut route, &mut invocation, &Default::default(), |id| {
        store.get(id)?.ok_or_else(|| "missing".into())
    })
    .unwrap();
    assert_eq!(
        invocation.integration_params["env"]["WORK_CONTEXT"],
        VARIABLE
    );
    assert_eq!(invocation.integration_params["env"]["RESEARCH_KEY"], SECRET);
    let mut child = mutate_child(root.path(), "revoke");
    wait_for(&root.path().join("mutation-started"));
    assert!(!root.path().join("mutation-committed").exists());
    // A real adapter process receives the complete frame, then waits for the
    // mutation to finish before responding. Holding the guard through response
    // would deadlock or make the mutation time out.
    let script = root.path().join("adapter.sh");
    fs::write(&script, format!("IFS= read -r request\nprintf '%s' \"$request\" > '{}'/received\nwhile [ ! -f '{}'/mutation-committed ]; do /bin/sleep 0.01; done\nprintf '%s\\n' '{{\"type\":\"result\",\"invocation_ref\":\"{INVOCATION}\",\"sequence\":1,\"status\":200,\"summary\":\"complete\",\"result\":{{}}}}'\n", root.path().display(), root.path().display())).unwrap();
    invocation.local_inputs = None;
    invocation.retained_route = None;
    let adapter = ResolvedProcessAdapter::new("/bin/sh")
        .with_argument(&script)
        .with_current_directory(root.path());
    let outcome = run_process_adapter_guarded(
        &adapter,
        invocation,
        &Default::default(),
        AdapterOutputSinks {
            model_output: &AdapterModelOutputMailbox::discard(),
            timing: &AdapterTimingSink::default(),
        },
        |_| {},
        |_| Ok(()),
        |_| Err("no calls".into()),
        guard,
    )
    .unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
    assert!(child.wait().unwrap().success());
    let received = fs::read_to_string(root.path().join("received")).unwrap();
    assert!(received.contains(SECRET) && received.contains(VARIABLE));
    assert!(!received.contains("local_inputs"));
    let mut revoked = retained.clone();
    let mut revoked_route = revoked.retained_route.clone().unwrap();
    assert!(handoff::prepare_with(
        &mut revoked_route,
        &mut revoked,
        &Default::default(),
        |_| panic!("revoked input fetched")
    )
    .is_err());
}

#[path = "lifecycle_tests.rs"]
mod lifecycle_tests;

#[path = "handoff_failure_tests.rs"]
mod failure_tests;

#[path = "retention_tests.rs"]
mod retention_tests;

#[path = "grant_tests.rs"]
mod grant_tests;

#[path = "supplied_tests.rs"]
mod supplied_tests;
