use std::{
    fs,
    os::unix::fs::PermissionsExt,
    thread,
    time::{Duration, Instant},
};

use cwr::v2026_09::host::{HostCommand, HostEvent, HOST_COMMAND_VERSION};

use super::{
    send,
    support::{
        configured_runtime_directory, start_configured_runtime_for, stop_configured_resident,
    },
    workspace,
};

#[test]
fn host_stdio_connects_to_a_resident_runtime_that_owns_the_adapter() {
    let temporary = tempfile::tempdir().expect("temporary configured runtime");
    let workspace = workspace::managed_workspace(temporary.path());
    let state_directory = temporary.path().join("configured-state");
    fs::create_dir_all(&state_directory).expect("create configured state directory");
    let legacy_database = state_directory.join("runtime.sqlite3");
    fs::write(&legacy_database, b"legacy database").expect("write legacy database sentinel");
    let communication_adapter = temporary.path().join("communication-adapter.sh");
    let marker = temporary.path().join("communication-adapter.started");
    fs::write(
        &communication_adapter,
        "#!/bin/sh\ntouch \"$1\"\nexec sleep 30\n",
    )
    .expect("write communication adapter");
    fs::set_permissions(&communication_adapter, fs::Permissions::from_mode(0o700))
        .expect("make communication adapter executable");
    let (mut child, mut input, output) = start_configured_runtime_for(
        temporary.path(),
        &workspace,
        &communication_adapter,
        &marker,
    );

    let selected = send(
        &mut input,
        &output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-configured-runtime".into(),
            actor_ref: crate::test_refs::actor_ref("root").as_str().into(),
        },
    );
    assert!(matches!(selected, HostEvent::ActorSelected { actor, .. }
        if actor.actor_ref == crate::test_refs::actor_ref("root").as_str()));
    // Host listeners can answer before the configured adapters have started.
    // Allow the same startup window as the resident launcher under suite load.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        marker.exists(),
        "configured communication adapter must start: {}",
        crate::resident_diagnostic_text(
            temporary
                .path()
                .join("configured-state/diagnostics/wlpdb.sqlite3")
        )
        .unwrap_or_default()
    );
    assert!(temporary
        .path()
        .join("configured-state/cwpdb.sqlite3")
        .exists());
    assert_eq!(
        fs::read(&legacy_database).expect("read legacy database sentinel"),
        b"legacy database"
    );
    let runtime = configured_runtime_directory(temporary.path());
    assert!(runtime.join("resident-runtime.pid").exists());

    drop(input);
    assert!(child
        .0
        .wait()
        .expect("wait for first configured bridge shutdown")
        .success());
    assert!(runtime.join("resident-runtime.pid").exists());

    let (mut replacement, mut replacement_input, replacement_output) = start_configured_runtime_for(
        temporary.path(),
        &workspace,
        &communication_adapter,
        &marker,
    );
    let selected = send(
        &mut replacement_input,
        &replacement_output,
        HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-replacement-bridge".into(),
            actor_ref: crate::test_refs::actor_ref("root").as_str().into(),
        },
    );
    assert!(matches!(selected, HostEvent::ActorSelected { actor, .. }
        if actor.actor_ref == crate::test_refs::actor_ref("root").as_str()));
    drop(replacement_input);
    assert!(replacement
        .0
        .wait()
        .expect("wait for replacement configured bridge shutdown")
        .success());
    assert!(runtime.join("resident-runtime.pid").exists());
    let stopped = std::process::Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
        .arg(temporary.path().join("configured-runtime.toml"))
        .arg("--stop-resident")
        .output()
        .expect("stop the configured resident");
    let pid_removed = !runtime.join("resident-runtime.pid").exists();
    // Clean up even if the command regresses.
    if !pid_removed {
        stop_configured_resident(temporary.path());
    }
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(
        pid_removed,
        "stop command must remove the configured resident PID"
    );
}

#[test]
fn planned_restart_exits_with_a_running_communication_adapter() {
    use cwr::v2026_09::service::{
        RuntimeServiceCommand, RuntimeServiceEvent, RUNTIME_SERVICE_VERSION,
    };
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let temporary = tempfile::tempdir().unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = cwr::v2026_09::local::stop_local_resident(
                &self.0.join("resident-runtime.pid"),
                &self.0.join("host.sock"),
            );
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let runtime = configured_runtime_directory(temporary.path());
    let _cleanup = Cleanup(runtime.clone());
    let workspace = workspace::managed_workspace(temporary.path());
    let adapter = temporary.path().join("communication-adapter.sh");
    let marker = temporary.path().join("communication-adapter.pid");
    fs::write(&adapter, "#!/bin/sh\necho $$ > \"$1\"\nexec sleep 120\n").unwrap();
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o700)).unwrap();
    let (_host, mut input, output) =
        start_configured_runtime_for(temporary.path(), &workspace, &adapter, &marker);
    assert!(matches!(
        send(
            &mut input,
            &output,
            HostCommand::SelectActor {
                version: HOST_COMMAND_VERSION,
                request_id: "select-before-restart".into(),
                actor_ref: crate::test_refs::actor_ref("root"),
            }
        ),
        HostEvent::ActorSelected { .. }
    ));
    // Ready follows capability probes, before the bridge subscribes. Wait for
    // its initial snapshot so the restart notice reaches a subscribed host.
    while !matches!(
        super::receive(&output),
        HostEvent::ConversationSnapshot { .. }
    ) {}
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let adapter_pid = fs::read_to_string(&marker).unwrap_or_else(|error| {
        panic!(
            "configured communication adapter did not start: {error}; {}",
            crate::resident_diagnostic_text(
                temporary
                    .path()
                    .join("configured-state/diagnostics/wlpdb.sqlite3")
            )
            .unwrap_or_default()
        )
    });
    let stream = UnixStream::connect(runtime.join("runtime.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut owner = BufReader::new(stream);
    let mut ready = String::new();
    owner.read_line(&mut ready).unwrap();
    assert!(matches!(
        serde_json::from_str::<RuntimeServiceEvent>(&ready).unwrap(),
        RuntimeServiceEvent::Ready { .. }
    ));
    let generation = format!("2 2 {} 123", "a".repeat(64));
    for operation in ["begin", "finish"] {
        serde_json::to_writer(
            owner.get_mut(),
            &RuntimeServiceCommand::DevGenerationRestart {
                version: RUNTIME_SERVICE_VERSION,
                request_id: operation.into(),
                operation: operation.into(),
                generation: generation.clone(),
            },
        )
        .unwrap();
        owner.get_mut().write_all(b"\n").unwrap();
        let mut line = String::new();
        owner.read_line(&mut line).unwrap();
        assert!(matches!(
            serde_json::from_str::<RuntimeServiceEvent>(&line).unwrap(),
            RuntimeServiceEvent::DrainStatus { remaining: 0, .. }
        ));
    }
    drop(owner);
    loop {
        if let HostEvent::RuntimeRestarting {
            generation: received,
        } = super::receive(&output)
        {
            assert_eq!(received, generation);
            break;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while runtime.join("resident-runtime.pid").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let exited = !runtime.join("resident-runtime.pid").exists();
    assert!(
        exited,
        "planned restart must exit despite the running adapter"
    );
    assert!(
        !std::process::Command::new("kill")
            .args(["-0", adapter_pid.trim()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success(),
        "resident shutdown must reap its adapter"
    );
}
