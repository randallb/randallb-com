#[path = "support/refs.rs"]
mod test_refs;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use cwr::v2026_09::host::{HostCommand, HostEvent, HOST_COMMAND_VERSION};

const RUNTIME_REF: &str = "cwp://example.test/runtime";
const SOURCE_REF: &str = "cwp://example.test/runtime/sources/conversations";
const WORKSPACE_ID: &str = "5f0dc30e-41bd-4f99-a8e4-0fb66caef76f";
const PERSON_ONLY_MANIFEST: &str = "type = 'cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath = './coworkers/person/COWORKER.toml'\nas = 'person'\n";

#[test]
fn resident_runtime_serves_two_actor_bound_host_sessions() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let mut runtime = start_runtime(temporary.path());
    let socket_path = temporary.path().join("state/host.sock");
    let workspace_directory = fs::canonicalize(fixture_workspace().parent().unwrap())
        .expect("canonical fixture workspace");
    let mut first = connect(
        &socket_path,
        &workspace_directory,
        "cwp://example.test/integrations/cli",
    );
    let mut second = connect(
        &socket_path,
        &workspace_directory,
        "cwp://example.test/integrations/tui",
    );

    first.open("person", crate::test_refs::actor_ref("person").as_str());
    second.open("helper", crate::test_refs::actor_ref("helper").as_str());

    first.send(&HostCommand::CreateConversation {
        version: HOST_COMMAND_VERSION,
        request_id: "reload".into(),
        source_ref: SOURCE_REF.into(),
        author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        text: "@cwr reload".into(),
    });
    let HostEvent::CommandSucceeded { receipt, .. } = first.wait_for("reload") else {
        panic!("@cwr reload must be admitted as retained conversation work")
    };
    assert!(receipt.invocation_ref.is_some());
    first.wait_for_conversation(&receipt.conversation_ref);
    second.wait_for_conversation(&receipt.conversation_ref);

    first.send(&HostCommand::CreateConversation {
        version: HOST_COMMAND_VERSION,
        request_id: "first-message".into(),
        source_ref: SOURCE_REF.into(),
        author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        text: "@helper Keep this shared note".into(),
    });
    let HostEvent::CommandSucceeded { receipt, .. } = first.wait_for("first-message") else {
        panic!("the first host must create the conversation")
    };
    let conversation_ref = receipt.conversation_ref;
    first.wait_for_conversation(&conversation_ref);
    second.wait_for_conversation(&conversation_ref);

    second.send(&HostCommand::ContinueConversation {
        version: HOST_COMMAND_VERSION,
        request_id: "impersonated-message".into(),
        source_ref: SOURCE_REF.into(),
        conversation_ref: conversation_ref.clone(),
        author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        text: "This must not be admitted".into(),
        recovery_only: false,
    });
    assert!(matches!(
        second.wait_for("impersonated-message"),
        HostEvent::CommandFailed { error, .. }
            if error.contains("cannot submit as")
    ));

    drop(first);
    second.send(&HostCommand::ContinueConversation {
        version: HOST_COMMAND_VERSION,
        request_id: "second-message".into(),
        source_ref: SOURCE_REF.into(),
        conversation_ref: conversation_ref.clone(),
        author_actor_ref: crate::test_refs::actor_ref("helper").as_str().into(),
        text: "The second host is still connected".into(),
        recovery_only: false,
    });
    assert!(matches!(
        second.wait_for("second-message"),
        HostEvent::CommandSucceeded { receipt, .. }
            if receipt.conversation_ref == conversation_ref
    ));
    assert!(runtime
        .0
        .try_wait()
        .expect("inspect resident runtime")
        .is_none());
    drop(second);
    // Admission inspection opens a managed writer, so release the resident's
    // writer lease before opening it rather than racing live publications.
    drop(runtime);
    let store = cwr::v2026_09::conversation::ConversationAdmissionStore::open_managed(
        temporary.path().join("state/cwpdb.sqlite3"),
        RUNTIME_REF,
    )
    .unwrap();
    let integrations = store
        .admissions()
        .map(|admission| admission.integration_ref.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(integrations.contains("cwp://example.test/integrations/cli"));
    assert!(integrations.contains("cwp://example.test/integrations/tui"));
}

#[test]
fn resident_runtime_rejects_a_host_for_another_workspace_path() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let _runtime = start_runtime(temporary.path());
    let socket_path = temporary.path().join("state/host.sock");
    let wrong_workspace = fs::canonicalize(temporary.path()).expect("canonical wrong workspace");
    let mut stream = connect_stream(&socket_path);
    serde_json::to_writer(
        &mut stream,
        &serde_json::json!({
        "version": 1,
        "workspace_id": WORKSPACE_ID,
        "workspace_path": wrong_workspace,
        "ingress_integration_ref": "cwp://example.test/integrations/test-client",
        }),
    )
    .expect("encode workspace handshake");
    stream.write_all(b"\n").expect("finish workspace handshake");
    stream.flush().expect("flush workspace handshake");

    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .expect("read workspace rejection");
    let response: serde_json::Value =
        serde_json::from_str(&response).expect("decode workspace rejection");
    assert_eq!(response["kind"], "workspace_rejected");
    assert!(response["error"]
        .as_str()
        .is_some_and(|error| error.contains("resident runtime owns workspace")));
}

#[test]
fn resident_runtime_exits_when_its_workspace_moved() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let original = temporary.path().join("original-workspace");
    copy_directory(fixture_workspace().parent().unwrap(), &original);
    let mut runtime = start_runtime_for_workspace(temporary.path(), &original.join("CWR.toml"));
    let socket_path = temporary.path().join("state/host.sock");
    drop(connect_stream(&socket_path));
    let moved = temporary.path().join("moved-workspace");
    fs::rename(&original, &moved).expect("move workspace");
    let moved = fs::canonicalize(moved).expect("canonical moved workspace");
    let mut stream = connect_stream(&socket_path);
    serde_json::to_writer(
        &mut stream,
        &serde_json::json!({
            "version": 1,
            "workspace_id": WORKSPACE_ID,
            "workspace_path": moved,
            "ingress_integration_ref": "cwp://example.test/integrations/test-client",
        }),
    )
    .expect("encode moved-workspace handshake");
    stream.write_all(b"\n").expect("finish workspace handshake");
    stream.flush().expect("flush workspace handshake");

    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .expect("read moved-workspace response");
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["kind"], "workspace_moved");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if runtime
            .0
            .try_wait()
            .expect("inspect moved-workspace resident")
            .is_some()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "moved-workspace resident must exit"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn resident_runtime_preserves_a_command_pipelined_after_workspace_binding() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let _runtime = start_runtime(temporary.path());
    let socket_path = temporary.path().join("state/host.sock");
    let workspace_directory = fs::canonicalize(fixture_workspace().parent().unwrap())
        .expect("canonical fixture workspace");
    let mut stream = connect_stream(&socket_path);
    let mut pipelined = serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "workspace_id": WORKSPACE_ID,
        "workspace_path": workspace_directory,
        "ingress_integration_ref": "cwp://example.test/integrations/test-client",
    }))
    .unwrap();
    pipelined.push(b'\n');
    pipelined.extend(
        serde_json::to_vec(&HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "pipelined-inspect".into(),
        })
        .unwrap(),
    );
    pipelined.push(b'\n');
    stream.write_all(&pipelined).expect("write pipelined input");
    stream.flush().expect("flush pipelined input");

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).unwrap();
    let binding_response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(binding_response["kind"], "workspace_bound");
    response.clear();
    reader.read_line(&mut response).unwrap();
    assert!(matches!(
        serde_json::from_str::<HostEvent>(&response).unwrap(),
        HostEvent::WorkspaceInspected { request_id, .. } if request_id == "pipelined-inspect"
    ));
}

#[test]
fn new_host_session_uses_the_resident_graph_until_reload() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let workspace = temporary.path().join("workspace");
    copy_directory(fixture_workspace().parent().unwrap(), &workspace);
    crate::test_refs::select_workspace(&workspace.join("CWR.toml"));
    fs::write(workspace.join("CWR.toml"), PERSON_ONLY_MANIFEST).unwrap();
    let person = workspace.join("coworkers/person/COWORKER.toml");
    let mut declaration: toml::Value =
        toml::from_str(&fs::read_to_string(&person).unwrap()).unwrap();
    declaration.as_table_mut().unwrap().remove("imports");
    fs::write(person, toml::to_string_pretty(&declaration).unwrap()).unwrap();
    let _runtime = start_runtime_for_workspace(temporary.path(), &workspace.join("CWR.toml"));
    let socket_path = temporary.path().join("state/host.sock");
    let workspace = fs::canonicalize(workspace).unwrap();

    drop(connect_stream(&socket_path));
    fs::copy(fixture_workspace(), workspace.join("CWR.toml")).unwrap();
    let mut client = connect(
        &socket_path,
        &workspace,
        "cwp://example.test/integrations/cli",
    );
    client.send(&HostCommand::InspectWorkspace {
        version: HOST_COMMAND_VERSION,
        request_id: "inspect-retained-graph".into(),
    });
    let HostEvent::WorkspaceInspected { workspace, .. } = client.wait_for("inspect-retained-graph")
    else {
        panic!("resident must inspect its retained graph")
    };
    assert_eq!(
        workspace
            .actors
            .iter()
            .map(|actor| actor.actor_ref.as_str())
            .collect::<Vec<_>>(),
        [
            crate::test_refs::actor_ref("person").as_str(),
            cwr::action_contract::INTRINSIC_CWR_ACTOR_REF
        ]
    );

    client.send(&HostCommand::SelectActor {
        version: HOST_COMMAND_VERSION,
        request_id: "select-unreloaded-helper".into(),
        actor_ref: crate::test_refs::actor_ref("helper").as_str().into(),
    });
    assert!(matches!(
        client.wait_for("select-unreloaded-helper"),
        HostEvent::CommandFailed { error, .. }
            if error.contains("is not in the resolved graph")
    ));
}

struct HostClient {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl HostClient {
    fn open(&mut self, request_slug: &str, actor_ref: &str) {
        let inspect_request = format!("inspect-{request_slug}");
        self.send(&HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: inspect_request.clone(),
        });
        assert!(matches!(
            self.wait_for(&inspect_request),
            HostEvent::WorkspaceInspected { .. }
        ));

        let select_request = format!("select-{request_slug}");
        self.send(&HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: select_request.clone(),
            actor_ref: actor_ref.into(),
        });
        assert!(matches!(
            self.wait_for(&select_request),
            HostEvent::ActorSelected { actor, .. } if actor.actor_ref == actor_ref
        ));

        let mut ready = false;
        let mut snapshot = false;
        while !(ready && snapshot) {
            match self.receive() {
                HostEvent::Ready { sources } => {
                    ready = sources.iter().any(|source| source.source_ref == SOURCE_REF)
                }
                HostEvent::ConversationSnapshot { source_ref, .. } => {
                    snapshot = source_ref == SOURCE_REF
                }
                _ => {}
            }
        }
    }

    fn send(&mut self, command: &HostCommand) {
        serde_json::to_writer(&mut self.stream, command).expect("encode host command");
        self.stream.write_all(b"\n").expect("finish host command");
        self.stream.flush().expect("flush host command");
    }

    fn wait_for(&mut self, request_id: &str) -> HostEvent {
        loop {
            let event = self.receive();
            let event_request_id = match &event {
                HostEvent::WorkspaceInspected { request_id, .. }
                | HostEvent::ActorSelected { request_id, .. }
                | HostEvent::CommandSucceeded { request_id, .. }
                | HostEvent::IntrinsicResponse { request_id, .. }
                | HostEvent::CommandFailed { request_id, .. } => Some(request_id.as_str()),
                _ => None,
            };
            if event_request_id == Some(request_id) {
                return event;
            }
        }
    }

    fn wait_for_conversation(&mut self, expected_ref: &str) {
        loop {
            if matches!(
                self.receive(),
                HostEvent::ConversationUpsert { conversation, .. }
                    if conversation.conversation_ref == expected_ref
            ) {
                return;
            }
        }
    }

    fn receive(&mut self) -> HostEvent {
        let mut line = String::new();
        assert_ne!(
            self.reader.read_line(&mut line).expect("read host event"),
            0,
            "host socket remains connected"
        );
        let event: HostEvent = serde_json::from_str(&line).expect("typed host event");
        event.validate().expect("valid host event");
        event
    }
}

fn connect(path: &Path, workspace_directory: &Path, ingress_integration_ref: &str) -> HostClient {
    let mut stream = connect_stream(path);
    serde_json::to_writer(
        &mut stream,
        &serde_json::json!({
            "version": 1,
            "workspace_id": WORKSPACE_ID,
            "workspace_path": workspace_directory,
            "ingress_integration_ref": ingress_integration_ref,
        }),
    )
    .expect("encode workspace handshake");
    stream.write_all(b"\n").expect("finish workspace handshake");
    stream.flush().expect("flush workspace handshake");
    let mut response = String::new();
    BufReader::new(stream.try_clone().expect("clone handshake socket"))
        .read_line(&mut response)
        .expect("read workspace handshake response");
    let response: serde_json::Value =
        serde_json::from_str(&response).expect("decode workspace handshake response");
    assert_eq!(response["kind"], "workspace_bound");
    HostClient {
        reader: BufReader::new(stream.try_clone().expect("clone host socket")),
        stream,
    }
}

fn connect_stream(path: &Path) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match UnixStream::connect(path) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .expect("bound host reads");
                return stream;
            }
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Err(error) => panic!("connect to resident host socket: {error}"),
        }
    }
}

fn start_runtime(directory: &Path) -> RuntimeProcess {
    start_runtime_for_workspace(directory, &fixture_workspace())
}

fn start_runtime_for_workspace(directory: &Path, workspace: &Path) -> RuntimeProcess {
    crate::test_refs::select_workspace(workspace);
    let config = directory.join("runtime.toml");
    fs::write(
        &config,
        format!(
            "version = 1\nworkspace = {:?}\nstate_directory = './state'\nruntime_directory = './state'\nworkspace_id = '{WORKSPACE_ID}'\nruntime_root_ref = '{RUNTIME_REF}'\nacting_actor_ref = {:?}\n",
            workspace.to_string_lossy(),
            crate::test_refs::actor_ref("person"),
        ),
    )
    .expect("write runtime configuration");
    RuntimeProcess(
        Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            // These host fixtures inspect legacy admission records directly.
            .env("WORKLOOP_CONFIGURATION", r#"{}"#)
            .arg(config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("launch resident runtime"),
    )
}

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create copied workspace directory");
    for entry in fs::read_dir(source).expect("read fixture workspace") {
        let entry = entry.expect("read fixture entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("inspect fixture entry").is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy fixture file");
        }
    }
}

fn fixture_workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace/CWR.toml")
}

struct RuntimeProcess(Child);

impl Drop for RuntimeProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
