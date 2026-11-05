use super::{configured_runtime::write_workspace, support::runtime_home};
use cwr::v2026_09::host::{HostCommand, HostEvent, HOST_COMMAND_VERSION};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn configured_observation_requires_an_existing_matching_owner_without_reloading_sources() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = write_workspace(temporary.path(), 1);
    let composition = cwr::v2026_09::local::LocalWorkspaceComposition::load(&workspace).unwrap();
    let expected = composition.runtimes[0].runtime_ref.clone();
    let selection = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
        .arg(&workspace)
        .arg("--only-runtime-selection")
        .env("XDG_STATE_HOME", temporary.path().join("state-home"))
        .env("XDG_RUNTIME_DIR", runtime_home(temporary.path()))
        .output()
        .unwrap();
    assert!(selection.status.success());
    let selection: serde_json::Value = serde_json::from_slice(&selection.stdout).unwrap();
    let directory = std::path::PathBuf::from(selection["runtime_directory"].as_str().unwrap());
    let observe = |expected: &str| {
        Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            .arg(&directory)
            .args([
                "--configured-runtime-control",
                "--observe-resident",
                expected,
            ])
            .env("WORKLOOP_CONFIGURATION", r#"{}"#)
            .env("XDG_STATE_HOME", temporary.path().join("state-home"))
            .env("XDG_RUNTIME_DIR", runtime_home(temporary.path()))
            .output()
            .unwrap()
    };
    assert!(!observe(&expected).status.success());
    assert!(!directory.join("resident-runtime.pid").exists());
    let started = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
        .arg(&workspace)
        .args(["--runtime", "installed-0", "--ensure-resident"])
        .env("WORKLOOP_CONFIGURATION", r#"{}"#)
        .env("XDG_STATE_HOME", temporary.path().join("state-home"))
        .env("XDG_RUNTIME_DIR", runtime_home(temporary.path()))
        .output()
        .unwrap();
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    // Existing-owner control must survive a broken candidate source tree.
    fs::write(
        temporary.path().join("package/CWR.toml"),
        "invalid candidate source",
    )
    .unwrap();
    let wrong_owner = observe("cwp://local.other/runtimes/00000000-0000-4000-8000-000000000001");
    let output = observe(&expected);
    // Always stop the fixture owner before checking the observation assertions.
    let stopped = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
        .arg(&directory)
        .args(["--configured-runtime-control", "--stop-resident"])
        .env("XDG_STATE_HOME", temporary.path().join("state-home"))
        .env("XDG_RUNTIME_DIR", runtime_home(temporary.path()))
        .output()
        .unwrap();
    assert!(stopped.status.success());
    assert!(!wrong_owner.status.success());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: cwr::v2026_09::local::LocalResidentObservation =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report.runtime_ref, expected);
    assert!(report.owner_pid > 1);
    assert_eq!(report.remaining, 0);
    assert!(!observe(&expected).status.success());
}

#[test]
fn configured_retirement_returns_only_an_exact_completed_owner_operation() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = write_workspace(temporary.path(), 1);
    let composition = cwr::v2026_09::local::LocalWorkspaceComposition::load(&workspace).unwrap();
    let expected_ref = composition.runtimes[0].runtime_ref.clone();
    let command = |args: &[&std::ffi::OsStr]| {
        Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            .args(args)
            .env("WORKLOOP_CONFIGURATION", r#"{}"#)
            .env("XDG_STATE_HOME", temporary.path().join("state-home"))
            .env("XDG_RUNTIME_DIR", runtime_home(temporary.path()))
            .output()
            .unwrap()
    };
    let output = command(&[workspace.as_os_str(), "--only-runtime-selection".as_ref()]);
    assert!(output.status.success());
    let selected: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let directory = std::path::PathBuf::from(selected["runtime_directory"].as_str().unwrap());
    let started = command(&[
        workspace.as_os_str(),
        "--runtime".as_ref(),
        "installed-0".as_ref(),
        "--ensure-resident".as_ref(),
    ]);
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let before = configured_history(temporary.path(), &workspace, &expected_ref, true);
    assert_eq!(before.len(), 3);
    let prior = command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--resident-autostart-state".as_ref(),
    ]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&prior.stdout).unwrap()["paused"],
        false
    );
    let paused = command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--set-resident-autostart-paused".as_ref(),
        "true".as_ref(),
    ]);
    assert!(paused.status.success());
    let observed = command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--observe-resident".as_ref(),
        expected_ref.as_ref(),
    ]);
    assert!(observed.status.success());
    let owner: cwr::v2026_09::local::LocalResidentObservation =
        serde_json::from_slice(&observed.stdout).unwrap();
    let expected = cwr::v2026_09::local::LocalResidentRetirement {
        version: 1,
        nonce: uuid::Uuid::new_v4().to_string(),
        runtime_ref: owner.runtime_ref,
        owner_pid: owner.owner_pid,
        owner_uid: owner.owner_uid,
    };
    let operation = serde_json::to_string(&expected).unwrap();
    // The fixture controller retains its unpredictable operation identity before send.
    let pending = temporary.path().join("controller-pending.json");
    fs::write(&pending, &operation).unwrap();
    fs::set_permissions(
        &pending,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    let retired = command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--retire-resident".as_ref(),
        operation.as_ref(),
    ]);
    if !retired.status.success() {
        let _ = command(&[
            directory.as_os_str(),
            "--configured-runtime-control".as_ref(),
            "--stop-resident".as_ref(),
        ]);
    }
    assert!(
        retired.status.success(),
        "{}",
        String::from_utf8_lossy(&retired.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&retired.stdout).unwrap()["completed"],
        true
    );
    let forbidden_start = command(&[
        workspace.as_os_str(),
        "--runtime".as_ref(),
        "installed-0".as_ref(),
        "--ensure-resident".as_ref(),
    ]);
    assert!(
        !forbidden_start.status.success(),
        "retired owner must stay paused until controller restores prior state"
    );
    let resumed = command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--set-resident-autostart-paused".as_ref(),
        "false".as_ref(),
    ]);
    assert!(resumed.status.success());
    let after = configured_history(temporary.path(), &workspace, &expected_ref, false);
    assert_eq!(
        after, before,
        "all paginated conversation refs and complete message records must survive retirement/reopen"
    );
    let stopped_replacement = command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--stop-resident".as_ref(),
    ]);
    assert!(stopped_replacement.status.success());
    let replay = command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--retirement-result".as_ref(),
        operation.as_ref(),
    ]);
    assert!(replay.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&replay.stdout).unwrap()["completed"],
        true
    );
    let mut other = expected;
    other.nonce = uuid::Uuid::new_v4().to_string();
    let other = serde_json::to_string(&other).unwrap();
    let mismatch = command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--retirement-result".as_ref(),
        other.as_ref(),
    ]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&mismatch.stdout).unwrap()["completed"],
        false
    );
    assert!(!command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--observe-resident".as_ref(),
        expected_ref.as_ref()
    ])
    .status
    .success());
}

fn configured_history(
    root: &Path,
    workspace: &Path,
    runtime_ref: &str,
    create: bool,
) -> Vec<(String, String)> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
        .arg(workspace)
        .args(["--runtime", "installed-0", "--host-stdio"])
        .env("WORKLOOP_CONFIGURATION", r#"{}"#)
        .env("CWR_CODEX_EXECUTABLE", root.join("no-provider-executable"))
        .env("CWR_CLAUDE_EXECUTABLE", root.join("no-provider-executable"))
        .env("XDG_STATE_HOME", root.join("state-home"))
        .env("XDG_RUNTIME_DIR", runtime_home(root))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut request = |command: HostCommand| -> HostEvent {
        let expected = command.request_id().to_owned();
        serde_json::to_writer(&mut input, &command).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(output.read_line(&mut line).unwrap() > 0);
            let event: HostEvent = serde_json::from_str(&line).unwrap();
            let value = serde_json::to_value(&event).unwrap();
            if value["request_id"] == expected {
                return event;
            }
        }
    };
    let inspected = request(HostCommand::InspectWorkspace {
        version: HOST_COMMAND_VERSION,
        request_id: "inspect-history".into(),
    });
    let HostEvent::WorkspaceInspected {
        default_actor_ref: Some(actor),
        ..
    } = inspected
    else {
        panic!("fixture workspace inspection failed")
    };
    assert!(matches!(
        request(HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-history".into(),
            actor_ref: actor.clone()
        }),
        HostEvent::ActorSelected { .. }
    ));
    let source_ref = format!("{runtime_ref}/sources/conversations");
    if create {
        for index in 0..3 {
            let result = request(HostCommand::CreateConversation {
                version: HOST_COMMAND_VERSION,
                request_id: format!("retain-{index}"),
                source_ref: source_ref.clone(),
                author_actor_ref: actor.clone(),
                text: format!("Fictional retained message {index}"),
            });
            assert!(
                matches!(result, HostEvent::CommandSucceeded { .. }),
                "{result:?}"
            );
        }
    }
    let mut retained = Vec::new();
    let mut after = None;
    loop {
        let event = request(HostCommand::ReadRootConversations {
            version: HOST_COMMAND_VERSION,
            request_id: format!("history-page-{}", retained.len()),
            source_ref: source_ref.clone(),
            page: cwr::v2026_09::conversation::ConversationSummaryPageRequest {
                first: Some(1),
                after,
                ..Default::default()
            },
            timeout_ms: 2000,
        });
        let HostEvent::RootConversationsPage { page, .. } = event else {
            panic!("history page unavailable: {event:?}")
        };
        after = page.summaries.last().map(|summary| summary.key.clone());
        for summary in page.summaries {
            retained.push((
                summary.key.conversation_ref,
                summary.last_message_preview.unwrap_or_default(),
            ));
        }
        if !page.has_next_page {
            break;
        }
        assert!(after.is_some() && retained.len() < 100);
    }
    let mut complete = Vec::new();
    for (conversation_index, (conversation_ref, _)) in retained.into_iter().enumerate() {
        let mut messages = Vec::new();
        let mut after = None;
        loop {
            let event = request(HostCommand::ReadConversationRecords {
                version: HOST_COMMAND_VERSION,
                request_id: format!("message-page-{conversation_index}-{}", messages.len()),
                source_ref: source_ref.clone(),
                read: cwr::v2026_09::conversation::ConversationRecordReadRequest::Content {
                    conversation_ref: conversation_ref.clone(),
                    page: cwr::v2026_09::conversation::ConversationRecordPageRequest {
                        first: Some(1),
                        after,
                        ..Default::default()
                    },
                },
                timeout_ms: 2000,
            });
            let HostEvent::ConversationRecordsPage {
                page: cwr::v2026_09::conversation::ConversationRecordPage::Content { page },
                ..
            } = event
            else {
                panic!("message content unavailable: {event:?}")
            };
            after = page.content.last().map(|item| item.key.clone());
            for content in page.content {
                messages.push(content.message);
            }
            if !page.has_next_page {
                break;
            }
            assert!(after.is_some() && messages.len() < 100);
        }
        assert!(!messages.is_empty());
        complete.push((conversation_ref, serde_json::to_string(&messages).unwrap()));
    }
    drop(input);
    assert!(child.wait().unwrap().success());
    complete.sort();
    complete
}

#[test]
fn armed_normal_idle_exit_publishes_only_the_saved_operation() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = write_workspace(temporary.path(), 1);
    let command = |args: &[&std::ffi::OsStr]| {
        Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            .args(args)
            .env("WORKLOOP_CONFIGURATION", "{}")
            .env("XDG_STATE_HOME", temporary.path().join("state-home"))
            .env("XDG_RUNTIME_DIR", runtime_home(temporary.path()))
            .output()
            .unwrap()
    };
    let selected = command(&[workspace.as_os_str(), "--only-runtime-selection".as_ref()]);
    let selected: serde_json::Value = serde_json::from_slice(&selected.stdout).unwrap();
    let directory = std::path::PathBuf::from(selected["runtime_directory"].as_str().unwrap());
    let runtime_ref = selected["runtime_ref"].as_str().unwrap();
    let started = command(&[
        workspace.as_os_str(),
        "--runtime".as_ref(),
        "installed-0".as_ref(),
        "--ensure-resident".as_ref(),
    ]);
    assert!(started.status.success());
    let observed = command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--observe-resident".as_ref(),
        runtime_ref.as_ref(),
    ]);
    let owner: cwr::v2026_09::local::LocalResidentObservation =
        serde_json::from_slice(&observed.stdout).unwrap();
    let expected = cwr::v2026_09::local::LocalResidentRetirement {
        version: 1,
        nonce: uuid::Uuid::new_v4().to_string(),
        runtime_ref: owner.runtime_ref,
        owner_pid: owner.owner_pid,
        owner_uid: owner.owner_uid,
    };
    let operation = serde_json::to_string(&expected).unwrap();
    let paused = command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--set-resident-autostart-paused".as_ref(),
        "true".as_ref(),
    ]);
    assert!(paused.status.success());
    for _ in 0..2 {
        let armed = command(&[
            directory.as_os_str(),
            "--configured-runtime-control".as_ref(),
            "--arm-retirement".as_ref(),
            operation.as_ref(),
        ]);
        assert!(
            armed.status.success(),
            "{}",
            String::from_utf8_lossy(&armed.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&armed.stdout).unwrap()["armed"],
            true
        );
    }
    let mut conflict = expected;
    conflict.nonce = uuid::Uuid::new_v4().to_string();
    let conflict = serde_json::to_string(&conflict).unwrap();
    assert!(!command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--arm-retirement".as_ref(),
        conflict.as_ref()
    ])
    .status
    .success());
    let exited = command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--exit-resident-when-idle".as_ref(),
    ]);
    assert!(exited.status.success());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let result = command(&[
            directory.as_os_str(),
            "--configured-runtime-control".as_ref(),
            "--retirement-result".as_ref(),
            operation.as_ref(),
        ]);
        assert!(result.status.success());
        if serde_json::from_slice::<serde_json::Value>(&result.stdout).unwrap()["completed"] == true
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "normal zero-work exit did not finalize its armed operation"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let other = command(&[
        directory.as_os_str(),
        "--configured-runtime-control".as_ref(),
        "--retirement-result".as_ref(),
        conflict.as_ref(),
    ]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&other.stdout).unwrap()["completed"],
        false
    );
    assert!(!command(&[
        workspace.as_os_str(),
        "--runtime".as_ref(),
        "installed-0".as_ref(),
        "--ensure-resident".as_ref()
    ])
    .status
    .success());
}
