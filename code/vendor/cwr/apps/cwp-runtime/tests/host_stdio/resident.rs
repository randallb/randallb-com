use super::*;

#[test]
fn resident_startup_honors_a_concurrent_workloop_environment_requirement() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let workspace = temporary.path().join("workspace/CWR.toml");
    let old_codex = temporary.path().join("old-codex");
    let new_codex = temporary.path().join("new-codex");

    let (mut probe, probe_input, _) = start_runtime(temporary.path());
    drop(probe_input);
    probe.0.kill().unwrap();
    probe.0.wait().unwrap();
    let required = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
        .arg(&workspace)
        .arg("--ensure-resident-environment")
        .env("WORKLOOP_CONFIGURATION", r#"{}"#)
        .env("XDG_STATE_HOME", temporary.path().join("state-home"))
        .env("XDG_RUNTIME_DIR", runtime_home(temporary.path()))
        .env("CWR_CODEX_EXECUTABLE", &new_codex)
        .output()
        .unwrap();
    assert!(required.status.success());

    let (mut old, mut old_input, old_output) =
        start_runtime_with_codex(temporary.path(), Some(&old_codex));
    let _ = send(
        &mut old_input,
        &old_output,
        HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "inspect-old-environment".into(),
        },
    );
    assert!(matches!(
        send(
            &mut old_input,
            &old_output,
            HostCommand::SelectActor {
                version: HOST_COMMAND_VERSION,
                request_id: "select-old-environment".into(),
                actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            },
        ),
        HostEvent::CommandFailed { .. }
    ));
    drop(old_input);
    assert!(!old.0.wait().unwrap().success());

    let (mut current, mut current_input, current_output) =
        start_runtime_with_codex(temporary.path(), Some(&new_codex));
    let _ = send(
        &mut current_input,
        &current_output,
        HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "inspect-current-environment".into(),
        },
    );
    assert!(matches!(
        send(
            &mut current_input,
            &current_output,
            HostCommand::SelectActor {
                version: HOST_COMMAND_VERSION,
                request_id: "select-current-environment".into(),
                actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            },
        ),
        HostEvent::ActorSelected { .. }
    ));
    drop(current_input);
    current.0.kill().unwrap();
    current.0.wait().unwrap();
    stop_resident(temporary.path());
}

#[test]
fn resident_environment_reuses_a_match_and_replaces_a_mismatch() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let old_codex = temporary.path().join("old-codex");
    let new_codex = temporary.path().join("new-codex");
    let (mut child, mut input, output) =
        start_runtime_with_codex(temporary.path(), Some(&old_codex));
    let _ = send(
        &mut input,
        &output,
        HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "inspect-environment".into(),
        },
    );
    let _ = send(
        &mut input,
        &output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-environment".into(),
            actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        },
    );
    drop(input);
    assert!(child.0.wait().unwrap().success());

    let runtime = runtime_directory(temporary.path());
    let original_pid = fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap();
    for codex in [&old_codex, &new_codex] {
        let result = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            .arg(temporary.path().join("workspace/CWR.toml"))
            .arg("--ensure-resident-environment")
            .env("WORKLOOP_CONFIGURATION", r#"{}"#)
            .env("XDG_STATE_HOME", temporary.path().join("state-home"))
            .env("XDG_RUNTIME_DIR", runtime_home(temporary.path()))
            .env("CWR_CODEX_EXECUTABLE", codex)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        if codex == &old_codex {
            assert_eq!(
                fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap(),
                original_pid
            );
        }
    }
    assert!(!runtime.join("resident-runtime.pid").exists());
    fs::remove_dir_all(runtime_home(temporary.path())).expect("remove test runtime directory");
}

#[test]
fn concurrent_stdio_bridges_share_one_healthy_resident() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let (mut first, mut first_input, first_output) = start_runtime(temporary.path());
    let runtime = runtime_directory(temporary.path());
    let _ = send(
        &mut first_input,
        &first_output,
        HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "inspect-first".into(),
        },
    );
    let _ = send(
        &mut first_input,
        &first_output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-first".into(),
            actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        },
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let first_pid = loop {
        if let Ok(pid) = fs::read_to_string(runtime.join("resident-runtime.pid")) {
            break pid;
        }
        assert!(Instant::now() < deadline, "resident PID was not published");
        thread::sleep(Duration::from_millis(10));
    };
    let (mut second, mut second_input, second_output) = start_runtime(temporary.path());
    assert!(matches!(
        send(
            &mut second_input,
            &second_output,
            HostCommand::InspectWorkspace {
                version: HOST_COMMAND_VERSION,
                request_id: "inspect-second".into(),
            },
        ),
        HostEvent::WorkspaceInspected { .. }
    ));
    let _ = send(
        &mut second_input,
        &second_output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-second".into(),
            actor_ref: crate::test_refs::actor_ref("helper").as_str().into(),
        },
    );
    assert_eq!(
        fs::read_to_string(runtime.join("resident-runtime.pid")).unwrap(),
        first_pid
    );

    drop(first_input);
    first.0.kill().unwrap();
    first.0.wait().unwrap();
    assert!(second.0.try_wait().unwrap().is_none());

    drop(second_input);
    second.0.kill().unwrap();
    second.0.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(40);
    while runtime.join("resident-runtime.pid").exists() {
        assert!(Instant::now() < deadline, "unused resident did not exit");
        thread::sleep(Duration::from_millis(100));
    }

    // An automatic idle exit must leave autostart enabled.
    let (mut third, mut third_input, third_output) = start_runtime(temporary.path());
    let _ = send(
        &mut third_input,
        &third_output,
        HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "inspect-after-idle".into(),
        },
    );
    assert!(matches!(
        send(
            &mut third_input,
            &third_output,
            HostCommand::SelectActor {
                version: HOST_COMMAND_VERSION,
                request_id: "select-after-idle".into(),
                actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            },
        ),
        HostEvent::ActorSelected { .. }
    ));
    drop(third_input);
    assert!(third.0.wait().unwrap().success());
    stop_resident(temporary.path());
}

#[test]
fn explicit_idle_exit_retires_an_unused_resident_without_the_grace_period() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let (mut child, mut input, output) = start_runtime(temporary.path());
    let _ = send(
        &mut input,
        &output,
        HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "inspect-before-exit".into(),
        },
    );
    let _ = send(
        &mut input,
        &output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-before-exit".into(),
            actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        },
    );
    let runtime = runtime_directory(temporary.path());
    drop(input);
    assert!(child.0.wait().unwrap().success());

    let result = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
        .arg(temporary.path().join("workspace/CWR.toml"))
        .arg("--exit-resident-when-idle")
        .env("WORKLOOP_CONFIGURATION", r#"{}"#)
        .env("XDG_STATE_HOME", temporary.path().join("state-home"))
        .env("XDG_RUNTIME_DIR", runtime.ancestors().nth(3).unwrap())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.join("resident-runtime.pid").exists() {
        assert!(
            Instant::now() < deadline,
            "resident did not honor explicit idle exit"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn host_stdio_reloads_as_the_selected_actor() {
    let temporary = tempfile::tempdir().expect("temporary runtime configuration");
    let (mut child, mut input, output) = start_runtime(temporary.path());
    let _ = send(
        &mut input,
        &output,
        HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "inspect-for-helper".into(),
        },
    );
    let selected = send(
        &mut input,
        &output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-helper".into(),
            actor_ref: crate::test_refs::actor_ref("helper").as_str().into(),
        },
    );
    assert!(matches!(
        selected,
        HostEvent::ActorSelected { actor, .. } if actor.actor_ref == crate::test_refs::actor_ref("helper").as_str()
    ));
    while !matches!(receive(&output), HostEvent::ConversationSnapshot { .. }) {}

    let manifest = temporary.path().join("workspace/CWR.toml");
    let mut document: toml::Value =
        toml::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
    document["imports"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| entry["as"].as_str() != Some("person"));
    let contents = toml::to_string_pretty(&document).unwrap();
    fs::write(manifest, contents).unwrap();
    let result = send(
        &mut input,
        &output,
        HostCommand::ReloadGraph {
            version: HOST_COMMAND_VERSION,
            request_id: "reload-as-helper".into(),
            source_ref: None,
        },
    );
    assert!(matches!(
        result,
        HostEvent::CommandSucceeded { request_id, .. } if request_id == "reload-as-helper"
    ));

    drop(input);
    assert!(child.0.wait().unwrap().success());
    stop_resident(temporary.path());
}

#[test]
fn resident_drain_commands_round_trip_and_persist_explicit_stop() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path();
    let (mut child, mut input, output) = start_runtime(directory);
    let _ = send(
        &mut input,
        &output,
        HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "inspect-drain".into(),
        },
    );
    let _ = send(
        &mut input,
        &output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-drain".into(),
            actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        },
    );
    let runtime = runtime_directory(directory);
    let control = |operation: &str| {
        let result = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            .arg(directory.join("workspace/CWR.toml"))
            .args(["--drain-resident", operation])
            .env("WORKLOOP_CONFIGURATION", r#"{}"#)
            .env("XDG_STATE_HOME", directory.join("state-home"))
            .env("XDG_RUNTIME_DIR", runtime.ancestors().nth(3).unwrap())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "0");
    };
    control("begin");
    control("status");
    control("cancel");
    control("begin");
    control("finish");
    control("pause-autostart");
    assert!(runtime.join("resident-stopped").is_file());
    control("resume-autostart");
    assert!(!runtime.join("resident-stopped").exists());
    drop(input);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    stop_resident(directory);
}

#[test]
fn host_stdio_eof_leaves_active_work_with_the_resident() {
    let temporary = tempfile::tempdir().expect("temporary managed runtime");
    let adapter = temporary.path().join("long-running-adapter");
    let adapter_pid = temporary.path().join("adapter.pid");
    fs::write(
        &adapter,
        format!(
            "#!/bin/sh\nIFS= read -r invocation\nprintf '%s' \"$$\" > '{}'\nexec /bin/sleep 30\n",
            adapter_pid.display()
        ),
    )
    .expect("write long-running adapter");
    let mut permissions = fs::metadata(&adapter).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).expect("make adapter executable");

    let workspace = workspace::managed_workspace(temporary.path());
    let (mut child, mut input, output) =
        start_runtime_for(temporary.path(), &workspace, Some(&adapter));
    let selected = send(
        &mut input,
        &output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-root".into(),
            actor_ref: crate::test_refs::actor_ref("root").as_str().into(),
        },
    );
    assert!(matches!(selected, HostEvent::ActorSelected { actor, .. }
        if actor.actor_ref == crate::test_refs::actor_ref("root").as_str()));

    let admitted = send(
        &mut input,
        &output,
        HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "start-long-work".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("root").as_str().into(),
            text: "@sh /bin/sleep 30".into(),
        },
    );
    let HostEvent::CommandSucceeded {
        request_id,
        receipt,
    } = admitted
    else {
        panic!("long-running work must be admitted")
    };
    assert_eq!(request_id, "start-long-work");
    let invocation_ref = receipt
        .invocation_ref
        .expect("long-running admission invocation");

    // This verifies lifetime after EOF, not startup speed. Allow the same
    // startup window as the resident launcher when the suite runs concurrently.
    let deadline = Instant::now() + Duration::from_secs(10);
    let pid = loop {
        let result = fs::read_to_string(&adapter_pid)
            .map_err(|error| format!("read adapter PID: {error}"))
            .and_then(|value| {
                value
                    .parse::<u32>()
                    .map_err(|error| format!("parse adapter PID: {error}"))
            });
        match result {
            Ok(pid) => break pid,
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let log = crate::resident_diagnostic_text(
                    runtime_state_directory(temporary.path()).join("diagnostics/wlpdb.sqlite3"),
                )
                .unwrap_or_default();
                stop_resident(temporary.path());
                panic!("long-running adapter did not publish a valid PID: {error}\nresident log: {log}");
            }
        }
    };

    drop(input);
    assert!(child
        .0
        .wait()
        .expect("wait for resident client shutdown")
        .success());
    let status = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("inspect adapter process");
    assert!(
        status.success(),
        "resident work must outlive its host client"
    );

    stop_resident(temporary.path());

    let status = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("inspect stopped adapter process");
    assert!(!status.success(), "resident shutdown must reap its adapter");
    let state = ConversationAdmissionStore::open_managed(
        runtime_state_directory(temporary.path()).join("cwpdb.sqlite3"),
        RUNTIME_REF,
    )
    .unwrap();
    let invocation = state
        .invocation_record(&invocation_ref)
        .expect("retained long-running invocation");
    assert_eq!(invocation.state, ActionInvocationState::Stopped);
}

#[test]
fn host_stdio_eof_does_not_leave_newly_admitted_work_runnable() {
    let temporary = tempfile::tempdir().expect("temporary managed runtime");
    let workspace = workspace::managed_workspace(temporary.path());
    let (mut child, mut input, output) = start_runtime_for(temporary.path(), &workspace, None);
    let selected = send(
        &mut input,
        &output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-root-for-queued-stop".into(),
            actor_ref: crate::test_refs::actor_ref("root").as_str().into(),
        },
    );
    assert!(matches!(selected, HostEvent::ActorSelected { actor, .. }
        if actor.actor_ref == crate::test_refs::actor_ref("root").as_str()));
    while !matches!(receive(&output), HostEvent::ConversationSnapshot { .. }) {}

    write(
        &mut input,
        &HostCommand::CreateConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "close-after-admission".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("root").as_str().into(),
            text: "@sh /bin/sleep 30".into(),
        },
    );
    drop(input);

    assert!(child
        .0
        .wait()
        .expect("wait for resident client shutdown")
        .success());
    stop_resident(temporary.path());
    let state = ConversationAdmissionStore::open_managed(
        runtime_state_directory(temporary.path()).join("cwpdb.sqlite3"),
        RUNTIME_REF,
    )
    .unwrap();
    let invocations = state.invocation_records().collect::<Vec<_>>();
    assert!(!invocations.is_empty());
    assert!(invocations
        .iter()
        .all(|invocation| invocation.state == ActionInvocationState::Stopped));
}
