use super::*;
use std::{os::unix::net::UnixListener, process::Command};

#[test]
fn older_configured_resident_can_bind_after_the_old_ten_second_cutoff() {
    let temporary = tempfile::tempdir_in("/tmp").unwrap();
    let socket = temporary.path().join("host.sock");
    let log = temporary.path().join("diagnostics.sqlite3");
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
        move || {
            wait_for_configured_resident(child, &socket, &log).map_err(|error| error.to_string())
        }
    });
    thread::sleep(Duration::from_millis(10_500));
    let listener = UnixListener::bind(&socket).unwrap();
    let stream = waiter.join().unwrap().expect("late bind should connect");
    drop(stream);
    drop(listener);
    drop(input);
}

#[test]
fn older_configured_resident_reports_an_exited_child() {
    let temporary = tempfile::tempdir_in("/tmp").unwrap();
    let child = Command::new("/bin/sh")
        .args(["-c", "exit 7"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let error = wait_for_configured_resident(
        child,
        &temporary.path().join("host.sock"),
        &temporary.path().join("diagnostics.sqlite3"),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("exited during startup (exit status: 7)"),
        "{error}"
    );
}

#[test]
fn older_configured_resident_accepts_a_winning_peer_after_its_child_exits() {
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
    let stream = wait_for_configured_resident(
        child,
        &socket,
        &temporary.path().join("diagnostics.sqlite3"),
    )
    .expect("a connected peer is authoritative");
    drop(stream);
    drop(listener);
}
