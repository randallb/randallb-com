use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    process::{Command, Stdio},
};

use serde_json::Value;

#[test]
fn bundled_sms_action_forwards_exact_packet_and_returns_result() {
    let temporary = tempfile::tempdir().unwrap();
    let socket = temporary.path().join("bridge.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cwr"))
        .args(["sms-action", "receive--message"])
        .env("CWR_INVOCATION_REF", "cwp://example.test/invocations/one")
        .env("WORKLOOP_SMS_BRIDGE_SOCKET", &socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"id":"one"}"#)
        .unwrap();
    let (mut bridge, _) = listener.accept().unwrap();
    let mut line = String::new();
    BufReader::new(bridge.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    let packet: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        packet,
        serde_json::json!({
            "version": 1,
            "action_name": "receive--message",
            "invocation_ref": "cwp://example.test/invocations/one",
            "input": {"id": "one"}
        })
    );
    bridge
        .write_all(b"{\"ok\":true,\"result\":{\"accepted\":true}}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        serde_json::json!({"accepted":true})
    );
}

#[test]
fn bundled_sms_action_redacts_rejected_content() {
    let temporary = tempfile::tempdir().unwrap();
    let socket = temporary.path().join("bridge.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cwr"))
        .args(["sms-action", "deliver--message"])
        .env("CWR_INVOCATION_REF", "cwp://example.test/invocations/two")
        .env("WORKLOOP_SMS_BRIDGE_SOCKET", &socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"message":"private-sms-body"}"#)
        .unwrap();
    let (mut bridge, _) = listener.accept().unwrap();
    let mut line = String::new();
    BufReader::new(bridge.try_clone().unwrap())
        .read_line(&mut line)
        .unwrap();
    bridge.write_all(b"{\"ok\":false}\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-sms-body"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(&socket.display().to_string()));
}
