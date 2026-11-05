#[path = "support/refs.rs"]
mod test_refs;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use cwr::v2026_09::{
    conversation::{ConversationAdmissionState, ConversationAdmissionStore},
    host::{HostCommand, HostEvent, HOST_COMMAND_VERSION},
    local::{LocalHostIdentity, LocalRuntime, LocalRuntimeOptions},
};

const WORKSPACE_ID: &str = "5f0dc30e-41bd-4f99-a8e4-0fb66caef76f";
const RUNTIME_REF: &str = "cwp://local.test-mac/runtimes/5f0dc30e-41bd-4f99-a8e4-0fb66caef76f";
const SOURCE_REF: &str =
    "cwp://local.test-mac/runtimes/5f0dc30e-41bd-4f99-a8e4-0fb66caef76f/sources/conversations";

#[test]
fn host_stdio_retains_cwr_results_for_new_and_existing_conversations() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let (child, mut input, receiver) = start_runtime(temporary.path());

    let result = send(
        &mut input,
        &receiver,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "new-intrinsic".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "@cwr list coworkers".into(),
        },
    );
    let HostEvent::CommandSucceeded { receipt: first, .. } = result else {
        panic!("CWR text must return a normal conversation receipt")
    };
    assert!(first.invocation_ref.is_some());

    let note = send(
        &mut input,
        &receiver,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "new-note".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "Keep this note".into(),
        },
    );
    let HostEvent::CommandSucceeded { receipt, .. } = note else {
        panic!("ordinary text must still return an admission receipt")
    };
    let result = send(
        &mut input,
        &receiver,
        HostCommand::ContinueConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "continued-intrinsic".into(),
            source_ref: SOURCE_REF.into(),
            conversation_ref: receipt.conversation_ref.clone(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "@cwr list coworkers".into(),
            recovery_only: false,
        },
    );
    let HostEvent::CommandSucceeded {
        receipt: continued, ..
    } = result
    else {
        panic!("continued CWR text must return a normal conversation receipt")
    };
    assert_eq!(continued.conversation_ref, receipt.conversation_ref);

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let event = receive(&receiver);
        if matches!(
            event,
            HostEvent::ConversationUpsert { conversation, .. }
                if conversation.messages.iter().any(|message| {
                    message.author_actor_ref == cwr::action_contract::INTRINSIC_CWR_ACTOR_REF
                        && message.text.contains("Available coworkers:")
                })
        ) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "retained CWR reply must be published"
        );
    }

    drop(input);
    drop(child);
    let (restarted, mut input, receiver) = start_runtime(temporary.path());
    let replayed = send(
        &mut input,
        &receiver,
        HostCommand::ContinueConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "continued-intrinsic".into(),
            source_ref: SOURCE_REF.into(),
            conversation_ref: receipt.conversation_ref.clone(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "@cwr list coworkers".into(),
            recovery_only: true,
        },
    );
    let HostEvent::CommandSucceeded {
        receipt: replayed, ..
    } = replayed
    else {
        panic!("recovery must replay the retained continuation")
    };
    assert_eq!(replayed.conversation_ref, continued.conversation_ref);
    assert_eq!(replayed.message_ref, continued.message_ref);
    assert_eq!(replayed.invocation_ref, continued.invocation_ref);

    let missing = send(
        &mut input,
        &receiver,
        HostCommand::ContinueConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "missing-recovery".into(),
            source_ref: SOURCE_REF.into(),
            conversation_ref: receipt.conversation_ref.clone(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "This must not be admitted".into(),
            recovery_only: true,
        },
    );
    assert!(matches!(
        missing,
        HostEvent::CommandFailed { error, .. }
            if error.contains("has no retained admission")
    ));
    drop(input);
    drop(restarted);
    let state =
        ConversationAdmissionStore::open_managed(runtime_database(temporary.path()), RUNTIME_REF)
            .unwrap();
    assert_eq!(state.admissions().count(), 3);
    assert!(state.conversations().any(|conversation| {
        conversation.messages.iter().any(|message| {
            message.author_actor_ref == cwr::action_contract::INTRINSIC_CWR_ACTOR_REF
                && message.text.contains("Available coworkers:")
        })
    }));
}

#[test]
fn host_stdio_retains_a_host_local_conversation_exchange() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let (child, mut input, receiver) = start_runtime(temporary.path());

    let result = send(
        &mut input,
        &receiver,
        HostCommand::RecordHostExchange {
            version: HOST_COMMAND_VERSION,
            request_id: "tui-help".into(),
            source_ref: SOURCE_REF.into(),
            requesting_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            host_actor_ref: "cwp://com.example.host/actors/tui".into(),
            conversation_ref: None,
            request_text: "help".into(),
            response_text: "I can show TUI help.".into(),
        },
    );
    let HostEvent::CommandSucceeded { receipt, .. } = result else {
        panic!("host exchange must return a conversation receipt")
    };
    assert!(receipt.invocation_ref.is_none());

    let conversation = loop {
        let HostEvent::ConversationUpsert { conversation, .. } = receive(&receiver) else {
            continue;
        };
        if conversation.conversation_ref == receipt.conversation_ref {
            break conversation;
        }
    };
    assert_eq!(conversation.messages.len(), 2);
    assert_eq!(
        conversation.messages[0].author_actor_ref,
        crate::test_refs::actor_ref("person").as_str()
    );
    assert_eq!(
        conversation.messages[0].addressed_actor_refs,
        vec!["cwp://com.example.host/actors/tui"]
    );
    assert_eq!(
        conversation.messages[1].author_actor_ref,
        "cwp://com.example.host/actors/tui"
    );
    assert_eq!(
        conversation.messages[1].addressed_actor_refs,
        vec![crate::test_refs::actor_ref("person").as_str()]
    );
    assert_eq!(
        conversation.messages[1].reply_to_message_ref.as_deref(),
        Some(conversation.messages[0].message_ref.as_str())
    );

    drop(input);
    drop(child);
    let state =
        ConversationAdmissionStore::open_managed(runtime_database(temporary.path()), RUNTIME_REF)
            .unwrap();
    assert!(state
        .conversations()
        .any(|retained| retained == &conversation));
}

#[test]
fn host_stdio_debugs_a_recovered_turn_after_automatic_execution() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = prepare_workspace(temporary.path());
    let state_directory = runtime_state_directory(temporary.path());
    fs::create_dir_all(&state_directory).unwrap();
    let runtime = LocalRuntime::open_with_state_path_and_runtime_ref_and_options(
        &workspace,
        state_directory.join("cwpdb.sqlite3"),
        RUNTIME_REF,
        LocalHostIdentity::new(
            crate::test_refs::actor_ref("person").as_str(),
            "cwp://example.test/integrations/workloop-desktop",
        ),
        LocalRuntimeOptions {
            conversation_timeline_rollout:
                cwr::v2026_09::conversation::ConversationTimelineRollout::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let admission = runtime
        .submit_text_as(
            "cwp://example.test/integrations/workloop-desktop",
            "pending-before-restart",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@cwr list coworkers",
        )
        .unwrap();
    drop(runtime);

    let (child, mut input, receiver) = start_runtime(temporary.path());
    let deadline = Instant::now() + Duration::from_secs(10);
    let invocation_ref = admission.invocation_ref().unwrap().to_owned();
    let mut attempt = 0_u64;
    loop {
        attempt += 1;
        let result = send(
            &mut input,
            &receiver,
            HostCommand::CreateConversation {
                version: HOST_COMMAND_VERSION,
                request_id: format!("debug-{attempt}"),
                source_ref: SOURCE_REF.into(),
                author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
                text: format!("@cwr debug invocation {invocation_ref}"),
            },
        );
        let HostEvent::CommandSucceeded { receipt, .. } = result else {
            panic!("retained CWR debug must return a normal conversation receipt")
        };
        let debug_text = loop {
            let HostEvent::ConversationUpsert { conversation, .. } = receive(&receiver) else {
                continue;
            };
            if conversation.conversation_ref != receipt.conversation_ref {
                continue;
            }
            if let Some(message) = conversation.messages.iter().find(|message| {
                message.author_actor_ref == cwr::action_contract::INTRINSIC_CWR_ACTOR_REF
            }) {
                break message.text.clone();
            }
        };
        if debug_text.contains("\"admission_state\": \"completed\"") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "recovered work must finish: {debug_text}"
        );
        thread::sleep(Duration::from_millis(10));
    }

    drop(input);
    drop(child);
    let state =
        ConversationAdmissionStore::open_managed(runtime_database(temporary.path()), RUNTIME_REF)
            .unwrap();
    let recovered = state
        .admissions()
        .find(|record| record.invocation_ref.as_deref() == Some(invocation_ref.as_str()))
        .expect("recovered admission remains persisted");
    assert_eq!(recovered.state, ConversationAdmissionState::Completed);
}

fn fixture_workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace/CWR.toml")
}

// These fixtures inspect the legacy store and exercise legacy transfer.
fn start_runtime(directory: &Path) -> (RuntimeProcess, ChildStdin, HostEvents) {
    start_runtime_with_configuration(directory, r#"{}"#)
}

fn start_runtime_with_configuration(
    directory: &Path,
    configuration: &str,
) -> (RuntimeProcess, ChildStdin, HostEvents) {
    let workspace = prepare_workspace(directory);
    let runtime_home = runtime_home(directory);
    let mut child = RuntimeProcess {
        child: Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            .args([
                workspace.as_os_str(),
                std::ffi::OsStr::new("--host-stdio"),
                std::ffi::OsStr::new("--ingress"),
                std::ffi::OsStr::new("cwp://example.test/integrations/test-host"),
            ])
            .env("WORKLOOP_CONFIGURATION", configuration)
            .env("XDG_STATE_HOME", directory.join("state-home"))
            .env("XDG_RUNTIME_DIR", &runtime_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("launch real runtime host entry point"),
        runtime_home,
    };
    let mut input = child.child.stdin.take().unwrap();
    let output = child.child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            if sender.send(line.expect("read runtime output")).is_err() {
                break;
            }
        }
    });
    let receiver = HostEvents::new(receiver);
    let _ = send(
        &mut input,
        &receiver,
        HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "host-intrinsic-inspect".into(),
        },
    );
    let selected = send(
        &mut input,
        &receiver,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "host-intrinsic-select".into(),
            actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        },
    );
    assert!(matches!(selected, HostEvent::ActorSelected { .. }));
    (child, input, receiver)
}

fn prepare_workspace(directory: &Path) -> PathBuf {
    let workspace = directory.join("workspace");
    if !workspace.join("CWR.toml").exists() {
        copy_directory(fixture_workspace().parent().unwrap(), &workspace);
        fs::create_dir_all(workspace.join(".cwr")).unwrap();
        fs::write(
            workspace.join(".cwr/workspace.json"),
            serde_json::to_vec(&serde_json::json!({
                "version": 2,
                "workspace_id": WORKSPACE_ID,
                "runtime_root_ref": RUNTIME_REF,
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let manifest = workspace.join("CWR.toml");
    crate::test_refs::select_workspace(&manifest);
    manifest
}

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".cwr" {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn runtime_state_directory(directory: &Path) -> PathBuf {
    let application = if cfg!(any(target_os = "macos", target_os = "windows")) {
        "CWR"
    } else {
        "cwr"
    };
    directory
        .join("state-home")
        .join(application)
        .join("workspaces")
        .join(WORKSPACE_ID)
}

fn runtime_database(directory: &Path) -> PathBuf {
    runtime_state_directory(directory).join("cwpdb.sqlite3")
}

fn runtime_home(directory: &Path) -> PathBuf {
    PathBuf::from("/tmp").join(format!(
        "cwr-hi-{}-{}",
        std::process::id(),
        directory.file_name().unwrap().to_string_lossy()
    ))
}

fn send(input: &mut ChildStdin, output: &HostEvents, command: HostCommand) -> HostEvent {
    serde_json::to_writer(&mut *input, &command).unwrap();
    input.write_all(b"\n").unwrap();
    input.flush().unwrap();
    output.receipt(command.request_id())
}

fn receive(output: &HostEvents) -> HostEvent {
    output.receive()
}

#[path = "host_intrinsic/events.rs"]
mod events;
use events::HostEvents;

struct RuntimeProcess {
    child: Child,
    runtime_home: PathBuf,
}

impl Drop for RuntimeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let pid_path = self
            .runtime_home
            .join("cwr/workspaces")
            .join(WORKSPACE_ID)
            .join("resident-runtime.pid");
        if let Ok(pid) = fs::read_to_string(pid_path) {
            let _ = Command::new("kill").args(["-TERM", pid.trim()]).status();
        }
        let socket = self
            .runtime_home
            .join("cwr/workspaces")
            .join(WORKSPACE_ID)
            .join("runtime.sock");
        let deadline = Instant::now() + Duration::from_secs(10);
        while socket.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let _ = fs::remove_dir_all(&self.runtime_home);
    }
}

#[path = "host_intrinsic/upstream.rs"]
mod upstream;

#[path = "host_intrinsic/rename.rs"]
mod rename;

#[path = "host_intrinsic/archive.rs"]
mod archive;

#[path = "host_intrinsic/transfer.rs"]
mod transfer;
