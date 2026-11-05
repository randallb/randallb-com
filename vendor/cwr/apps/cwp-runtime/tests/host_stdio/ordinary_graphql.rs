//! Installed-client upgrade coverage: no development generation environment is used.
use super::*;
use cwr::v2026_09::local::{LocalGraphqlEndpoint, LocalWorkspacePaths};
use std::path::Path;

const INGRESS: &str = "cwp://example.test/integrations/tui-upgrade";

#[test]
#[ignore = "subprocess fixture for ordinary GraphQL startup tests"]
fn bootstrap_fixture() {
    let workspace = std::env::var_os("CWR_GRAPHQL_UPGRADE_WORKSPACE").expect("fixture workspace");
    let actor = std::env::var("CWR_GRAPHQL_UPGRADE_ACTOR").unwrap();
    let paths = LocalWorkspacePaths::resolve(workspace).unwrap();
    let _host = cwr::v2026_09::local::connect_or_start_local_graphql_resident(
        &paths,
        Some(&actor),
        Path::new(env!("CARGO_BIN_EXE_cwp-runtime")),
        INGRESS,
    )
    .unwrap()
    .unwrap();
    let endpoint =
        LocalGraphqlEndpoint::read(&paths.runtime_directory.join("graphql.json")).unwrap();
    let http = reqwest::blocking::Client::builder()
        .unix_socket(endpoint.socket_path.clone())
        .no_proxy()
        .build()
        .unwrap();
    let opened: serde_json::Value = http
        .post(format!("{}/session", endpoint.url()))
        .bearer_auth(&endpoint.token)
        .json(&serde_json::json!({"actorRef":actor,"ingressIntegrationRef":INGRESS}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(opened["actorRef"], actor);
    http.delete(format!("{}/session", endpoint.url()))
        .bearer_auth(opened["token"].as_str().unwrap())
        .send()
        .unwrap()
        .error_for_status()
        .unwrap();
}

#[test]
fn ordinary_graphql_bootstrap_reuses_a_capable_resident() {
    check_upgrade(Path::new(env!("CARGO_BIN_EXE_cwp-runtime")), false);
}

#[test]
#[ignore = "requires CWR_LEGACY_RUNTIME_EXECUTABLE built from revision ccaba8261f1c"]
fn ordinary_graphql_bootstrap_replaces_an_older_resident_and_retains_history() {
    let old = std::env::var_os("CWR_LEGACY_RUNTIME_EXECUTABLE").expect("old runtime executable");
    check_upgrade(Path::new(&old), true);
}

fn check_upgrade(executable: &Path, replaces: bool) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let workspace = workspace::managed_workspace(root);
    support::prepare_workspace_identity(&workspace);
    let actor = crate::test_refs::actor_ref("root");
    let paths = LocalWorkspacePaths::resolve_with_base_directories(
        &workspace,
        root.join("state-home"),
        runtime_home(root),
    )
    .unwrap();
    let command = |executable: &Path| {
        let mut command = Command::new(executable);
        command
            .env("WORKLOOP_CONFIGURATION", r#"{}"#)
            .env("XDG_STATE_HOME", root.join("state-home"))
            .env("XDG_RUNTIME_DIR", runtime_home(root))
            .env_remove("CWR_DEV_RUNTIME_GENERATION")
            .env_remove("CWR_RECONNECT_RUNTIME_GENERATION");
        command
    };
    struct Cleanup<'a>(&'a dyn Fn(&Path) -> Command, &'a Path);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = (self.0)(Path::new(env!("CARGO_BIN_EXE_cwp-runtime")))
                .arg(self.1)
                .arg("--stop-resident")
                .output();
        }
    }
    let _cleanup = Cleanup(&command, &workspace);
    let mut bridge = command(executable)
        .arg(&workspace)
        .arg("--host-stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = bridge.stdin.take().unwrap();
    let (sender, output) = std::sync::mpsc::channel();
    let stdout = bridge.stdout.take().unwrap();
    use std::io::BufRead;
    thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if sender.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    assert!(matches!(
        send(
            &mut input,
            &output,
            HostCommand::SelectActor {
                version: HOST_COMMAND_VERSION,
                request_id: "select".into(),
                actor_ref: actor.clone()
            }
        ),
        HostEvent::ActorSelected { .. }
    ));
    let HostEvent::CommandSucceeded { receipt, .. } = send(
        &mut input,
        &output,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "retained".into(),
            source_ref: format!("{}/sources/conversations", paths.runtime_root_ref),
            author_actor_ref: actor.clone(),
            text: "History survives installed client upgrades".into(),
        },
    ) else {
        panic!("message admission failed")
    };
    let pid = || fs::read_to_string(paths.runtime_directory.join("resident-runtime.pid")).unwrap();
    let before = pid();
    let bootstrap = || {
        let result = command(&std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ordinary_graphql::bootstrap_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("CWR_GRAPHQL_UPGRADE_WORKSPACE", &workspace)
            .env("CWR_GRAPHQL_UPGRADE_ACTOR", &actor)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    };
    bootstrap();
    let after = pid();
    assert_eq!(before != after, replaces);
    bootstrap();
    assert_eq!(after, pid());
    drop(_cleanup);
    let retained =
        ConversationAdmissionStore::open_managed(paths.database_path(), &paths.runtime_root_ref)
            .unwrap();
    assert!(retained
        .conversation(&receipt.conversation_ref)
        .unwrap()
        .messages
        .iter()
        .any(|message| message.text == "History survives installed client upgrades"));
    drop(retained);
    drop(input);
    let _ = bridge.kill();
    let _ = bridge.wait();
}
