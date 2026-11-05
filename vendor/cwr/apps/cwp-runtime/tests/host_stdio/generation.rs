use super::*;
use cwr::v2026_09::service::{RuntimeServiceCommand, RuntimeServiceEvent, RUNTIME_SERVICE_VERSION};
use std::{
    io::{BufRead, BufReader},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::mpsc,
};

#[path = "generation_legacy.rs"]
mod legacy;

struct GenerationTest {
    directory: tempfile::TempDir,
    workspace: PathBuf,
    adapter: PathBuf,
    generation_file: PathBuf,
    first: PathBuf,
    second: PathBuf,
    first_identity: String,
    second_identity: String,
    actor: String,
}

impl GenerationTest {
    fn new() -> Self {
        Self::with_first_executable(std::path::Path::new(env!("CARGO_BIN_EXE_cwp-runtime")))
    }

    fn with_first_executable(executable: &std::path::Path) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let workspace = workspace::managed_workspace(root);
        support::prepare_workspace_identity(&workspace);
        let actor = crate::test_refs::actor_ref("root");
        let first = root.join("runtime-first");
        let second = root.join("runtime-second");
        fs::copy(executable, &first).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_cwp-runtime"), &second).unwrap();
        let first_identity = executable_identity(&first);
        let second_identity = executable_identity(&second);
        let adapter = root.join("adapter");
        fs::write(&adapter, include_str!("generation_adapter.py")).unwrap();
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
        let generation_file = root.join("generation");
        Self {
            directory,
            workspace,
            adapter,
            generation_file,
            first,
            second,
            first_identity,
            second_identity,
            actor,
        }
    }

    fn generation(&self, number: u32) -> String {
        let identity = if number == 1 {
            &self.first_identity
        } else {
            &self.second_identity
        };
        format!("1 {number} {identity}")
    }

    fn publish(&self, number: u32) {
        fs::write(&self.generation_file, self.generation(number)).unwrap();
    }

    fn start(
        &self,
        number: u32,
        reconnect: bool,
    ) -> (support::RuntimeProcess, ChildStdin, Receiver<String>) {
        let executable = if number == 1 || reconnect {
            &self.first
        } else {
            &self.second
        };
        let mut command = Command::new(executable);
        command
            .arg(&self.workspace)
            .args([
                "--host-stdio",
                "--ingress",
                "cwp://example.test/integrations/generation-test",
            ])
            .env("WORKLOOP_CONFIGURATION", r#"{}"#)
            .env("XDG_STATE_HOME", self.directory.path().join("state-home"))
            .env(
                "XDG_RUNTIME_DIR",
                support::runtime_home(self.directory.path()),
            )
            .env("CWR_DEV_RUNTIME_EXECUTABLE", executable)
            .env("CWR_DEV_RUNTIME_GENERATION", self.generation(number))
            .env("CWR_DEV_RUNTIME_GENERATION_FILE", &self.generation_file)
            .env("CWR_SH_ADAPTER_EXECUTABLE", &self.adapter)
            .env_remove("CWR_RECONNECT_RUNTIME_GENERATION")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.directory.path().join(format!("starter-{number}.log")))
                    .unwrap(),
            );
        if reconnect {
            command.env("CWR_RECONNECT_RUNTIME_GENERATION", self.generation(number));
        }
        let mut child = support::RuntimeProcess(command.spawn().unwrap());
        let input = child.0.stdin.take().unwrap();
        let stdout = child.0.stdout.take().unwrap();
        let (sender, output) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        (child, input, output)
    }

    fn select(&self, input: &mut ChildStdin, output: &Receiver<String>) -> HostEvent {
        send(input, output, self.selection())
    }

    fn selection(&self) -> HostCommand {
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-generation".into(),
            actor_ref: self.actor.clone(),
        }
    }

    fn control(&self) -> BufReader<UnixStream> {
        let stream =
            UnixStream::connect(runtime_directory(self.directory.path()).join("runtime.sock"))
                .unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        BufReader::new(stream)
    }

    fn submit(
        &self,
        input: &mut ChildStdin,
        output: &Receiver<String>,
        id: &str,
    ) -> cwr::v2026_09::host::HostCommandReceipt {
        let event = send(
            input,
            output,
            HostCommand::CreateConversation {
                version: HOST_COMMAND_VERSION,
                request_id: id.into(),
                source_ref: SOURCE_REF.into(),
                author_actor_ref: self.actor.clone(),
                text: format!("@sh {id}"),
            },
        );
        let HostEvent::CommandSucceeded { receipt, .. } = event else {
            panic!("{event:?}")
        };
        receipt
    }
}

fn executable_identity(executable: &std::path::Path) -> String {
    let hashed = Command::new("python3").args(["-c", "import hashlib,sys; print(hashlib.file_digest(open(sys.argv[1], 'rb'), 'sha256').hexdigest())"])
        .arg(executable).output().unwrap();
    assert!(hashed.status.success());
    format!(
        "{} {}",
        String::from_utf8(hashed.stdout).unwrap().trim(),
        fs::metadata(executable).unwrap().len()
    )
}

impl Drop for GenerationTest {
    fn drop(&mut self) {
        if std::thread::panicking() {
            if let Ok(entries) =
                fs::read_dir(runtime_state_directory(self.directory.path()).join("diagnostics"))
            {
                for entry in entries.flatten() {
                    eprintln!(
                        "diagnostic: {}",
                        fs::read_to_string(entry.path()).unwrap_or_default()
                    );
                }
            }
            eprintln!(
                "resident log: {}",
                crate::resident_diagnostic_text(
                    runtime_state_directory(self.directory.path())
                        .join("diagnostics/wlpdb.sqlite3")
                )
                .unwrap_or_default()
            );
        }
        let runtime = runtime_directory(self.directory.path());
        let _ = cwr::v2026_09::local::stop_local_resident(
            &runtime.join("resident-runtime.pid"),
            &runtime.join("host.sock"),
        );
        let _ = fs::remove_dir_all(support::runtime_home(self.directory.path()));
    }
}

fn control(
    reader: &mut BufReader<UnixStream>,
    command: RuntimeServiceCommand,
) -> RuntimeServiceEvent {
    let expected = command.request_id().to_owned();
    serde_json::to_writer(reader.get_mut(), &command).unwrap();
    reader.get_mut().write_all(b"\n").unwrap();
    loop {
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).unwrap(), 0);
        let event: RuntimeServiceEvent = serde_json::from_str(&line).unwrap();
        if matches!(&event, RuntimeServiceEvent::DrainStatus { request_id, .. } | RuntimeServiceEvent::CommandFailed { request_id, .. } if *request_id == expected)
        {
            return event;
        }
    }
}

#[track_caller]
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "generation transition did not complete"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn snapshot(output: &Receiver<String>) -> Vec<cwr::v2026_09::conversation::RetainedConversation> {
    loop {
        if let HostEvent::ConversationSnapshot { conversations, .. } = receive(output) {
            return conversations;
        }
    }
}

#[test]
fn generation_replacement_drains_work_preserves_history_and_starts_one_resident() {
    let test = GenerationTest::new();
    test.publish(1);
    let (_old, mut input, output) = test.start(1, false);
    assert!(matches!(
        test.select(&mut input, &output),
        HostEvent::ActorSelected { .. }
    ));
    let active = test.submit(&mut input, &output, "active");
    wait_until(|| {
        fs::read_to_string(test.directory.path().join("started")).is_ok_and(|text| !text.is_empty())
    });
    let runtime = runtime_directory(test.directory.path());
    let old_pid = fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap();
    // A connected host that never binds/selects has no restart notice to drain.
    // Keep it open throughout replacement; it must not hold the old resident.
    let _unselected = UnixStream::connect(runtime.join("host.sock")).unwrap();
    test.publish(2);
    let (_new, mut new_input, new_output) = test.start(2, false);
    write(&mut new_input, &test.selection());
    let (_competitor, mut competing_input, competing_output) = test.start(2, false);
    write(&mut competing_input, &test.selection());
    let mut observer = test.control();
    wait_until(|| {
        matches!(control(&mut observer, RuntimeServiceCommand::Drain {
        version: RUNTIME_SERVICE_VERSION, request_id: "probe".into(), operation: "status".into(),
    }), RuntimeServiceEvent::CommandFailed { error, .. } if error.contains("replacement already owns"))
    });
    drop(observer);
    let queued = test.submit(&mut input, &output, "queued");
    assert_eq!(
        fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap(),
        old_pid
    );
    assert_eq!(
        fs::read_to_string(test.directory.path().join("started"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    fs::write(test.directory.path().join("release"), "finish").unwrap();
    loop {
        if let HostEvent::RuntimeRestarting { generation } = receive(&output) {
            assert_eq!(generation, test.generation(2));
            break;
        }
    }
    let selected = receive(&new_output);
    assert!(
        matches!(selected, HostEvent::ActorSelected { .. }),
        "{selected:?}"
    );
    let selected = receive(&competing_output);
    assert!(
        matches!(selected, HostEvent::ActorSelected { .. }),
        "{selected:?}"
    );
    let new_pid = fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap();
    assert_ne!(old_pid, new_pid);
    assert_eq!(
        fs::read_to_string(runtime.join("dev-runtime-generation"))
            .unwrap()
            .trim(),
        test.generation(2)
    );
    let restored = snapshot(&new_output);
    assert!(restored
        .iter()
        .any(
            |conversation| conversation.conversation_ref == active.conversation_ref
                && conversation
                    .messages
                    .iter()
                    .any(|message| message.text.contains("Finished before restart"))
        ));
    assert!(restored
        .iter()
        .any(|conversation| conversation.conversation_ref == queued.conversation_ref));
    wait_until(|| {
        fs::read_to_string(test.directory.path().join("started"))
            .unwrap()
            .lines()
            .count()
            == 2
    });
    // An old executable follows the selected generation without replacing it.
    let (_reconnected, mut reconnect_input, reconnect_output) = test.start(1, true);
    let reselected = test.select(&mut reconnect_input, &reconnect_output);
    assert!(
        matches!(reselected, HostEvent::ActorSelected { .. }),
        "{reselected:?}"
    );
    assert!(!snapshot(&reconnect_output).is_empty());
    assert_eq!(
        fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap(),
        new_pid
    );
    let mut retained = cwr::v2026_09::conversation::ConversationReadStore::open(
        runtime_state_directory(test.directory.path()).join("cwpdb.sqlite3"),
        RUNTIME_REF,
    )
    .unwrap();
    let operation = retained.begin_operation(Duration::from_secs(10)).unwrap();
    assert_eq!(
        operation
            .conversation_invocation(
                &active.conversation_ref,
                active.invocation_ref.as_ref().unwrap()
            )
            .unwrap()
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );
}

#[test]
fn generation_requester_disconnect_releases_its_drain() {
    let test = GenerationTest::new();
    test.publish(1);
    let (_host, mut input, output) = test.start(1, false);
    assert!(matches!(
        test.select(&mut input, &output),
        HostEvent::ActorSelected { .. }
    ));
    let request = |operation: &str| RuntimeServiceCommand::DevGenerationRestart {
        version: RUNTIME_SERVICE_VERSION,
        request_id: "owner".into(),
        operation: operation.into(),
        generation: test.generation(2),
    };
    let mut owner = test.control();
    assert!(matches!(
        control(&mut owner, request("begin")),
        RuntimeServiceEvent::DrainStatus { remaining: 0, .. }
    ));
    let mut other = test.control();
    assert!(matches!(
        control(&mut other, request("cancel")),
        RuntimeServiceEvent::CommandFailed { .. }
    ));
    drop(owner);
    wait_until(|| {
        matches!(
            control(
                &mut other,
                RuntimeServiceCommand::Drain {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: "status".into(),
                    operation: "status".into()
                }
            ),
            RuntimeServiceEvent::DrainStatus { .. }
        )
    });
    fs::write(test.directory.path().join("release"), "finish").unwrap();
    test.submit(&mut input, &output, "after-abandoned-restart");
    wait_until(|| {
        fs::read_to_string(test.directory.path().join("started")).is_ok_and(|text| !text.is_empty())
    });
}

#[test]
fn generation_replacement_reports_startup_failure_without_starting_an_old_generation() {
    let test = GenerationTest::new();
    test.publish(1);
    let (_host, mut input, output) = test.start(1, false);
    assert!(matches!(
        test.select(&mut input, &output),
        HostEvent::ActorSelected { .. }
    ));
    fs::write(
        test.workspace.parent().unwrap().join(".cwr/runtime.toml"),
        "version = 1\nmax_concurrent_conversations = 0\n",
    )
    .unwrap();
    test.publish(2);
    let (_failed, mut next_input, next_output) = test.start(2, false);
    let failed = test.select(&mut next_input, &next_output);
    assert!(
        matches!(failed, HostEvent::CommandFailed { error, .. } if error.contains("exited during startup") && error.contains("diagnostics/wlpdb.sqlite3"))
    );
    let (_retry, mut retry_input, retry_output) = test.start(2, true);
    assert!(matches!(
        test.select(&mut retry_input, &retry_output),
        HostEvent::CommandFailed { .. }
    ));
    assert!(!runtime_directory(test.directory.path())
        .join("resident-runtime.pid")
        .exists());
}

#[test]
fn a_newer_build_during_drain_keeps_the_old_resident_and_completed_work() {
    let test = GenerationTest::new();
    test.publish(1);
    let (_host, mut input, output) = test.start(1, false);
    assert!(matches!(
        test.select(&mut input, &output),
        HostEvent::ActorSelected { .. }
    ));
    let active = test.submit(&mut input, &output, "active-before-newer-build");
    wait_until(|| {
        fs::read_to_string(test.directory.path().join("started")).is_ok_and(|text| !text.is_empty())
    });
    let runtime = runtime_directory(test.directory.path());
    let old_pid = fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap();
    test.publish(2);
    let (_next, mut next_input, next_output) = test.start(2, false);
    write(&mut next_input, &test.selection());
    let mut observer = test.control();
    wait_until(|| {
        matches!(control(&mut observer, RuntimeServiceCommand::Drain {
        version: RUNTIME_SERVICE_VERSION, request_id: "probe".into(), operation: "status".into(),
    }), RuntimeServiceEvent::CommandFailed { error, .. } if error.contains("replacement already owns"))
    });
    test.publish(3);
    fs::write(test.directory.path().join("release"), "finish").unwrap();
    assert!(
        matches!(receive(&next_output), HostEvent::CommandFailed { error, .. } if error.contains("rebuilt again"))
    );
    assert_eq!(
        fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap(),
        old_pid
    );
    let mut retained = cwr::v2026_09::conversation::ConversationReadStore::open(
        runtime_state_directory(test.directory.path()).join("cwpdb.sqlite3"),
        RUNTIME_REF,
    )
    .unwrap();
    let operation = retained.begin_operation(Duration::from_secs(10)).unwrap();
    assert_eq!(
        operation
            .conversation_invocation(
                &active.conversation_ref,
                active.invocation_ref.as_ref().unwrap()
            )
            .unwrap()
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );
}

#[test]
fn reconnect_rejects_a_stale_marker_for_a_different_resident_executable() {
    let mut test = GenerationTest::new();
    test.publish(1);
    let (_host, mut input, output) = test.start(1, false);
    assert!(matches!(
        test.select(&mut input, &output),
        HostEvent::ActorSelected { .. }
    ));
    let runtime = runtime_directory(test.directory.path());
    let old_pid = fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap();
    let (digest, size) = test.first_identity.split_once(' ').unwrap();
    let identities = [
        format!("{} {size}", "0".repeat(64)),
        format!("{digest} {}", size.parse::<u64>().unwrap() + 1),
    ];
    for identity in identities {
        // Failed startup can leave a marker even when a generation-unaware
        // launcher subsequently starts a different executable at this socket.
        test.second_identity = identity;
        fs::write(runtime.join("dev-runtime-generation"), test.generation(2)).unwrap();
        let (_retry, mut retry_input, retry_output) = test.start(2, true);
        let event = test.select(&mut retry_input, &retry_output);
        assert!(
            matches!(event, HostEvent::CommandFailed { ref error, .. }
            if error.contains("replacement resident executable does not match")),
            "{event:?}"
        );
        assert_eq!(
            fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap(),
            old_pid
        );
    }
    // Rejecting this reconnect must neither terminate nor replace the resident.
    assert!(matches!(
        send(
            &mut input,
            &output,
            HostCommand::ReloadGraph {
                version: HOST_COMMAND_VERSION,
                request_id: "still-serving-after-rejected-reconnect".into(),
                source_ref: Some(SOURCE_REF.into()),
            }
        ),
        HostEvent::CommandSucceeded { .. }
    ));
}

#[test]
fn reconnect_rejects_identical_runtime_bytes_from_the_previous_bundle() {
    let test = GenerationTest::new();
    test.publish(1);
    let (_host, mut input, output) = test.start(1, false);
    assert!(matches!(
        test.select(&mut input, &output),
        HostEvent::ActorSelected { .. }
    ));
    let runtime = runtime_directory(test.directory.path());
    // Adapter-only changes preserve runtime bytes but select a different bundle path.
    fs::write(runtime.join("dev-runtime-generation"), test.generation(2)).unwrap();
    fs::write(
        runtime.join("dev-runtime-executable"),
        fs::canonicalize(&test.second).unwrap().to_str().unwrap(),
    )
    .unwrap();
    let (_retry, mut retry_input, retry_output) = test.start(2, true);
    let event = test.select(&mut retry_input, &retry_output);
    assert!(
        matches!(event, HostEvent::CommandFailed { ref error, .. } if error.contains("different runtime bundle")),
        "{event:?}"
    );
}
