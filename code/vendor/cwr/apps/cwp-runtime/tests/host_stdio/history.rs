use super::*;
use cwr::v2026_09::conversation::{
    ConversationActivityEvent, ConversationRecordPage, ConversationRecordPageRequest,
    ConversationRecordReadRequest,
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader},
    sync::mpsc,
};

/// Read only on request: leaving this receiver idle really stalls the host pipe,
/// unlike an eager background reader feeding an unbounded channel.
struct HistoryHost {
    process: support::RuntimeProcess,
    input: ChildStdin,
    reads: mpsc::Sender<()>,
    output: Receiver<std::io::Result<String>>,
    directory: tempfile::TempDir,
}

impl HistoryHost {
    fn start() -> Self {
        Self::start_with_adapter(include_str!("history_adapter.py"))
    }

    fn start_with_adapter(script: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace::managed_workspace(directory.path());
        for file in ["sh-role.toml", "local-sh.toml"] {
            let path = workspace.parent().unwrap().join(file);
            fs::write(
                &path,
                fs::read_to_string(&path)
                    .unwrap()
                    .replace("/adapters/sh", "/adapters/codex"),
            )
            .unwrap();
        }
        let role = workspace.parent().unwrap().join("sh-role.toml");
        fs::OpenOptions::new()
            .append(true)
            .open(role)
            .unwrap()
            .write_all(b"\n[adapters.params]\nentrypoint = './PROMPT.md'\n")
            .unwrap();
        fs::write(
            workspace.parent().unwrap().join("PROMPT.md"),
            "Restore the retained conversation.",
        )
        .unwrap();
        support::prepare_workspace_identity(&workspace);
        let adapter = directory.path().join("adapter");
        // These protocol fixtures stand in for the adapter, including its
        // out-of-band storage configuration entry point.
        let script = script.replacen(
            '\n',
            r#"
import json, os, pathlib, sys
if sys.argv[1:] == ["--configure"]:
    data = pathlib.Path(os.environ["CWR_ADAPTER_DATA_DIRECTORY"])
    home = data / "provider-home"
    home.mkdir(mode=0o700, exist_ok=True)
    print(json.dumps({"data_directory": str(data), "provider_home": str(home)}))
    sys.exit(0)
"#,
            1,
        );
        fs::write(&adapter, script).unwrap();
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
        // Route readiness and resolution require a signed-in Codex executable
        // before launching our fake adapter. Supply one without depending on a
        // provider install or account.
        // This CLI is stateless: keep one source path rather than generating a fresh
        // executable for each host. Workspace, adapter and provider state stay isolated.
        let codex =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/host_stdio/fake_codex.sh");
        // The adapter verifies that its selected CLI is this fixture, via samefile.
        std::os::unix::fs::symlink(&codex, directory.path().join("codex")).unwrap();
        let search_path = std::env::join_paths(std::iter::once(directory.path().to_owned()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        let mut process = support::RuntimeProcess(
            Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
                .arg(&workspace)
                .args([
                    "--host-stdio",
                    "--ingress",
                    "cwp://example.test/integrations/history-test",
                ])
                .env("WORKLOOP_CONFIGURATION", r#"{}"#)
                .env("XDG_STATE_HOME", directory.path().join("state-home"))
                .env("XDG_RUNTIME_DIR", support::runtime_home(directory.path()))
                .env(
                    "CWR_ADAPTER_DATA_ROOT",
                    directory.path().join("adapter-data"),
                )
                .env(
                    "CWR_CODEX_SOURCE_HOME",
                    directory.path().join("empty-source"),
                )
                .env("CWR_CODEX_ADAPTER_EXECUTABLE", &adapter)
                .env("CWR_CODEX_EXECUTABLE", &codex)
                .env("PATH", search_path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(fs::File::create(directory.path().join("host.log")).unwrap())
                .spawn()
                .unwrap(),
        );
        let input = process.0.stdin.take().unwrap();
        let stdout = process.0.stdout.take().unwrap();
        let (reads, requests) = mpsc::channel();
        let (sender, output) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while requests.recv().is_ok() {
                let mut line = String::new();
                let result = reader.read_line(&mut line).map(|_| line);
                if sender.send(result).is_err() {
                    break;
                }
            }
        });
        let mut host = Self {
            process,
            input,
            reads,
            output,
            directory,
        };
        host.command(HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select".into(),
            actor_ref: crate::test_refs::actor_ref("root"),
        });
        while !matches!(host.receive(), HostEvent::ConversationSnapshot { .. }) {}
        host
    }

    fn receive(&self) -> HostEvent {
        self.reads.send(()).unwrap();
        let line = self
            .output
            .recv_timeout(Duration::from_secs(10))
            .expect("host output deadline")
            .unwrap();
        assert!(
            !line.is_empty(),
            "host disconnected during checkpoint restoration"
        );
        assert!(line.ends_with('\n'), "partial host frame");
        let event: HostEvent = serde_json::from_str(&line).unwrap();
        event.validate().unwrap();
        event
    }

    fn command(&mut self, command: HostCommand) -> Vec<HostEvent> {
        let request_id = command.request_id().to_owned();
        write(&mut self.input, &command);
        let mut events = Vec::new();
        loop {
            let event = self.receive();
            assert!(
                !matches!(&event, HostEvent::CommandFailed { .. }),
                "{event:?}"
            );
            let completed = matches!(&event, HostEvent::CommandSucceeded { request_id: id, .. } | HostEvent::ActorSelected { request_id: id, .. } if *id == request_id);
            events.push(event);
            if completed {
                return events;
            }
        }
    }

    fn barrier(&mut self, conversation_ref: &str, sequence: u64) -> Vec<HostEvent> {
        // This harmless status command is ordered after earlier publications on
        // the same connection and does not publish the conversation itself.
        self.command(HostCommand::SetConversationLiveStatus {
            version: HOST_COMMAND_VERSION,
            request_id: format!("barrier-{sequence}"),
            source_ref: SOURCE_REF.into(),
            conversation_ref: conversation_ref.into(),
            subject_actor_ref: crate::test_refs::actor_ref("root"),
            publisher_session_id: "history-test".into(),
            sequence,
            target_actor_refs: Vec::new(),
            status: None,
            lease_ms: 0,
        })
    }

    fn release(&self, name: &str) {
        fs::write(self.directory.path().join(name), "").unwrap();
    }

    fn wait_for(&self, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.directory.path().join(name).exists() {
            assert!(Instant::now() < deadline, "adapter did not reach {name}");
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for HistoryHost {
    fn drop(&mut self) {
        if thread::panicking() {
            if let Ok(entries) =
                fs::read_dir(runtime_state_directory(self.directory.path()).join("diagnostics"))
            {
                for entry in entries.flatten() {
                    eprintln!(
                        "{}: {}",
                        entry.path().display(),
                        fs::read_to_string(entry.path()).unwrap_or_default()
                    );
                }
            }
            for path in [
                self.directory.path().join("host.log"),
                runtime_state_directory(self.directory.path()).join("diagnostics/wlpdb.sqlite3"),
            ] {
                eprintln!(
                    "{}: {}",
                    path.display(),
                    fs::read_to_string(&path).unwrap_or_default()
                );
            }
        }
        let runtime = runtime_directory(self.directory.path());
        let _ = cwr::v2026_09::local::stop_local_resident(
            &runtime.join("resident-runtime.pid"),
            &runtime.join("host.sock"),
        );
        let _ = fs::remove_dir_all(support::runtime_home(self.directory.path()));
    }
}

fn receipt(events: &[HostEvent]) -> &cwr::v2026_09::host::HostCommandReceipt {
    let Some(HostEvent::CommandSucceeded { receipt, .. }) = events.last() else {
        panic!("missing receipt")
    };
    receipt
}

#[test]
fn checkpoint_restoration_preserves_history_and_a_delayed_host() {
    let mut host = HistoryHost::start();
    let retained_text = format!("Keep this history: {}", "h".repeat(384 * 1024));
    let admitted = host.command(HostCommand::CreateConversation {
        version: HOST_COMMAND_VERSION,
        request_id: "seed-history".into(),
        source_ref: SOURCE_REF.into(),
        author_actor_ref: crate::test_refs::actor_ref("root"),
        text: format!("@root {retained_text}"),
    });
    let conversation_ref = receipt(&admitted).conversation_ref.clone();
    let started = host.command(HostCommand::ContinueConversation {
        version: HOST_COMMAND_VERSION,
        request_id: "restore-history".into(),
        source_ref: SOURCE_REF.into(),
        author_actor_ref: crate::test_refs::actor_ref("root"),
        conversation_ref: conversation_ref.clone(),
        text: "@sh Restore the history".into(),
        recovery_only: false,
    });
    let invocation_ref = receipt(&started).invocation_ref.clone().unwrap();
    host.wait_for("ready");
    let initial = host.barrier(&conversation_ref, 1);
    let frame_bytes = initial
        .iter()
        .chain(&started)
        .filter(|event| matches!(event, HostEvent::ConversationUpsert { .. }))
        .map(|event| serde_json::to_vec(event).unwrap().len())
        .max()
        .unwrap();
    assert!(frame_bytes > 384 * 1024);
    eprintln!("retained conversation frame: {frame_bytes} bytes");

    // Pause consumption until all 67 checkpoints have been synchronously
    // acknowledged. No progress or final output is produced in this phase.
    let restoration_started = Instant::now();
    host.release("restore");
    host.wait_for("acknowledged.json");
    eprintln!(
        "67 checkpoints acknowledged in {:?} with the host reader paused",
        restoration_started.elapsed()
    );
    let checkpoint_events = host.barrier(&conversation_ref, 2);
    assert_eq!(
        checkpoint_events
            .iter()
            .filter(|event| matches!(event, HostEvent::ConversationUpsert { .. }))
            .count(),
        0,
        "checkpoint-only changes must not republish retained history"
    );

    host.release("visible");
    host.wait_for("visible-ready");
    let visible = host.barrier(&conversation_ref, 3);
    let visible = serde_json::to_string(&visible).unwrap();
    assert!(visible.contains("Conversation history restored: 33/33 parts."));
    assert!(
        visible.contains("echo history-restored"),
        "provider tool activity must remain visible"
    );
    host.release("finish");
    loop {
        if let HostEvent::ConversationExecutionFinished { response, .. } = host.receive() {
            assert_eq!(response.invocation_ref, invocation_ref);
            assert_eq!(
                response.result,
                Some(json!({"text": "Restoration finished"}))
            );
            break;
        }
    }
    host.barrier(&conversation_ref, 4);
    assert!(
        host.process.0.try_wait().unwrap().is_none(),
        "host must still be connected"
    );

    // Connection survival is established above. Stop this isolated resident
    // before opening the admission store, which can perform recovery writes.
    stop_resident(host.directory.path());
    let store = ConversationAdmissionStore::open_managed(
        runtime_state_directory(host.directory.path()).join("cwpdb.sqlite3"),
        RUNTIME_REF,
    )
    .unwrap();
    let retained = store.execution_conversation(&invocation_ref).unwrap();
    let checkpoints: Vec<_> = retained
        .activity
        .iter()
        .filter_map(|activity| match &activity.event {
            ConversationActivityEvent::ProviderEvent { method, data, .. }
                if method == "cwr.adapter_checkpoint" =>
            {
                Some(data.clone())
            }
            _ => None,
        })
        .collect();
    let acknowledged: Vec<Value> =
        serde_json::from_slice(&fs::read(host.directory.path().join("acknowledged.json")).unwrap())
            .unwrap();
    assert_eq!(
        checkpoints, acknowledged,
        "every acknowledged checkpoint must be retained"
    );
    assert_eq!(checkpoints.len(), 67);
    for (part, pair) in checkpoints[..66].chunks_exact(2).enumerate() {
        assert_eq!(
            (
                pair[0]["confirmed_items"].as_u64(),
                pair[0]["pending"].as_bool()
            ),
            (Some(part as u64), Some(true))
        );
        assert_eq!(
            (
                pair[1]["confirmed_items"].as_u64(),
                pair[1]["pending"].as_bool()
            ),
            (Some(part as u64 + 1), Some(false))
        );
        assert_eq!(pair[0]["live_started"], false);
        assert_eq!(pair[1]["live_started"], false);
    }
    assert_eq!(checkpoints[66]["live_started"], true);
    assert!(store
        .conversation(&conversation_ref)
        .unwrap()
        .messages
        .iter()
        .any(|message| message.text == retained_text));
}

#[test]
fn reads_conversation_content_without_replaying_full_history() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let (mut child, mut input, output) = start_runtime(temporary.path());
    write(
        &mut input,
        &HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-for-content".into(),
            actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        },
    );
    let mut selected = false;
    let mut ready = false;
    let mut snapshot = false;
    while !(selected && ready && snapshot) {
        match receive(&output) {
            HostEvent::ActorSelected { request_id, .. } if request_id == "select-for-content" => {
                selected = true;
            }
            HostEvent::Ready { .. } => ready = true,
            HostEvent::ConversationSnapshot { .. } => snapshot = true,
            _ => {}
        }
    }
    let created = send(
        &mut input,
        &output,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "create-for-content".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            text: "@cwr show help".into(),
        },
    );
    let HostEvent::CommandSucceeded { receipt, .. } = created else {
        panic!("conversation creation must return a receipt")
    };

    write(
        &mut input,
        &HostCommand::ReadConversationRecords {
            version: HOST_COMMAND_VERSION,
            request_id: "read-content".into(),
            source_ref: SOURCE_REF.into(),
            read: ConversationRecordReadRequest::Content {
                conversation_ref: receipt.conversation_ref,
                page: ConversationRecordPageRequest {
                    first: Some(1),
                    ..Default::default()
                },
            },
            timeout_ms: 1_000,
        },
    );
    drop(input);
    let event = loop {
        let event = receive(&output);
        match &event {
            HostEvent::ConversationSnapshot { .. } => {
                panic!("a record page read must not replay the full conversation snapshot")
            }
            HostEvent::ConversationRecordsPage { request_id, .. }
                if request_id == "read-content" =>
            {
                break event;
            }
            _ => {}
        }
    };
    let HostEvent::ConversationRecordsPage { page, .. } = event else {
        panic!("read command must return a conversation record page")
    };
    let ConversationRecordPage::Content { page } = page else {
        panic!("content read must return a content page")
    };
    assert_eq!(page.content.len(), 1);
    assert_eq!(page.content[0].message.text, "show help");

    assert!(child
        .wait_for_exit("wait for record read to drain after stdin closes")
        .success());
    stop_resident(temporary.path());
}

#[path = "history/delivery.rs"]
mod delivery;

include!("delegation.rs");
