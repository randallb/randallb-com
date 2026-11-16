use super::*;
#[test]
fn ordinary_host_handshake_is_accepted_by_the_previous_strict_decoder() {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PreviousHandshake {
        version: u32,
        selection: serde_json::Value,
        ingress: String,
    }
    let mut handshake = Handshake {
        version: 1,
        selection: SessionBinding {
            workspace_path: "/workspace/WORKSPACE.toml".into(),
            runtime_name: "test".into(),
            fingerprint: "fingerprint".into(),
            acting_actor_ref: "cwp://example.test/person".into(),
        },
        ingress: "cwp://example.test/tui".into(),
        ensure_only: false,
    };
    let ordinary = serde_json::to_value(&handshake).unwrap();
    let decoded: PreviousHandshake = serde_json::from_value(ordinary).unwrap();
    assert_eq!(decoded.version, 1);
    assert_eq!(decoded.selection["loop_name"], "test");
    assert_eq!(decoded.ingress, handshake.ingress);
    handshake.ensure_only = true;
    assert!(
        serde_json::from_value::<PreviousHandshake>(serde_json::to_value(handshake).unwrap())
            .is_err()
    );
}

#[test]
fn only_explicit_unsupported_probe_rejections_allow_replacement() {
    assert!(unsupported_ensure_probe(
        &serde_json::json!({"kind":"configured_loop_rejected", "error":"unknown field `ensure_only`, expected one of `version`, `selection`, `ingress` at line 1 column 42"})
    ));
    for error in ["configured runtime selection or executable configuration differs from the active resident; its work continues unchanged", "connection closed", "unknown field `other`, expected `version`"] {
        assert!(!unsupported_ensure_probe(&serde_json::json!({"kind":"configured_loop_rejected", "error":error})));
    }
}

#[test]
fn configured_resident_can_bind_after_the_old_ten_second_cutoff() {
    use std::{
        os::unix::net::UnixListener,
        process::{Command, Stdio},
        thread,
        time::Duration,
    };

    let temporary = tempfile::tempdir_in("/tmp").unwrap();
    let socket = temporary.path().join("host.sock");
    let log = temporary.path().join("diagnostics.sqlite3");
    // The pipe keeps the child alive until the launcher has connected.
    let mut child = Command::new("/bin/sh")
        .args(["-c", "read value"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let input = child.stdin.take().unwrap();
    let waiter = thread::spawn({
        let socket = socket.clone();
        move || wait_for_started_resident(child, &socket, &log).map_err(|error| error.to_string())
    });
    thread::sleep(Duration::from_millis(10_500));
    let listener = UnixListener::bind(&socket).unwrap();
    let stream = waiter.join().unwrap().expect("late bind should connect");
    drop(stream);
    drop(listener);
    drop(input);
}

#[test]
fn configured_resident_reports_an_exited_child() {
    use std::process::{Command, Stdio};

    let temporary = tempfile::tempdir_in("/tmp").unwrap();
    let child = Command::new("/bin/sh")
        .args(["-c", "exit 7"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let error = wait_for_started_resident(
        child,
        &temporary.path().join("host.sock"),
        &temporary.path().join("diagnostics.sqlite3"),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("exited (exit status: 7)"),
        "{error}"
    );
}

#[test]
fn configured_resident_accepts_a_winning_peer_after_its_child_exits() {
    use std::{
        os::unix::net::UnixListener,
        process::{Command, Stdio},
    };

    let temporary = tempfile::tempdir_in("/tmp").unwrap();
    let socket = temporary.path().join("host.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let mut child = Command::new("/bin/sh")
        .args(["-c", "exit 7"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(7));
    let stream = wait_for_started_resident(
        child,
        &socket,
        &temporary.path().join("diagnostics.sqlite3"),
    )
    .expect("a connected peer is authoritative");
    drop(stream);
    drop(listener);
}
