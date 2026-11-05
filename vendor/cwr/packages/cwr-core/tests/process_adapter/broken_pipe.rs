use super::*;

#[test]
fn broken_host_reply_waits_for_a_same_tick_terminal_handoff() {
    let (directory, executable) = fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"call-1","operation":"cwr.test","input":{{}}}}'
printf '%s\n' '{{"type":"result","invocation_ref":"{INVOCATION_REF}","sequence":1,"status":504,"summary":"Command timed out.","error":{{"code":"command_timeout"}}}}'
exit 0
"##,
    ));
    let adapter = adapter(&executable, &directory.path().join("unused"));
    let outcome = run_process_adapter_with_controlled_host_calls(
        &adapter,
        invocation(true),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |_| Ok(AdapterHostCallOutcome::Returned(json!({}))),
    )
    .expect("the reader must hand off a same-tick terminal after the reply pipe closes");

    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter was unexpectedly stopped")
    };
    assert_eq!(terminal.status, 504);
    assert_eq!(terminal.error.as_ref().unwrap().code, "command_timeout");
}

#[test]
fn broken_host_reply_waits_for_a_delayed_terminal_handoff() {
    let (directory, executable) = fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"call-1","operation":"cwr.test","input":{{}}}}'
exec 0<&-
sleep 0.4
printf '%s\n' '{{"type":"result","invocation_ref":"{INVOCATION_REF}","sequence":1,"status":504,"summary":"Command timed out.","error":{{"code":"command_timeout"}}}}'
exit 0
"##,
    ));
    let adapter = adapter(&executable, &directory.path().join("unused"));
    let outcome = run_process_adapter_with_controlled_host_calls(
        &adapter,
        invocation(true),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |_| Ok(AdapterHostCallOutcome::Returned(json!({}))),
    )
    .expect("the reader must hand off a delayed terminal after the reply pipe closes");

    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter was unexpectedly stopped")
    };
    assert_eq!(terminal.status, 504);
    assert_eq!(terminal.error.as_ref().unwrap().code, "command_timeout");
}

#[test]
fn broken_host_reply_pipe_preserves_an_already_emitted_terminal() {
    let (directory, executable) = fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"call-1","operation":"cwr.test","input":{{}}}}'
printf '%s\n' '{{"type":"result","invocation_ref":"{INVOCATION_REF}","sequence":1,"status":504,"summary":"Command timed out.","error":{{"code":"command_timeout","details":{{"stdout":"partial"}}}}}}'
exit 0
"##,
    ));
    let adapter = adapter(&executable, &directory.path().join("unused"));
    let outcome = run_process_adapter_with_controlled_host_calls(
        &adapter,
        invocation(true),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |_| {
            thread::sleep(Duration::from_millis(200));
            Ok(AdapterHostCallOutcome::Returned(json!({})))
        },
    )
    .expect("the queued terminal must survive a broken host reply pipe");
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter was unexpectedly stopped")
    };
    assert_eq!(terminal.status, 504);
    assert_eq!(terminal.error.as_ref().unwrap().code, "command_timeout");
    assert_eq!(
        terminal.error.as_ref().unwrap().details.as_ref().unwrap()["stdout"],
        "partial"
    );
}

#[test]
fn broken_host_reply_pipe_without_a_terminal_fails_promptly() {
    let (directory, executable) = fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
exec 0<&-
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"call-1","operation":"cwr.test","input":{{}}}}'
printf '%s\n' '{{"type":"progress","invocation_ref":"{INVOCATION_REF}","sequence":1,"status":102,"summary":"still running"}}'
sleep 30
"##,
    ));
    let adapter = adapter(&executable, &directory.path().join("unused"));
    let started = Instant::now();
    let error = run_process_adapter_with_controlled_host_calls(
        &adapter,
        invocation(true),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |_| Ok(AdapterHostCallOutcome::Returned(json!({}))),
    )
    .expect_err("a live adapter that closes its reply pipe must fail");

    assert!(
        started.elapsed() < Duration::from_secs(6),
        "broken reply pipe waited for the live adapter"
    );
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::BrokenPipe)
    );
}

#[test]
fn broken_host_reply_pipe_preserves_a_terminal_beyond_progress_capacity() {
    let (directory, executable) = fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"call-1","operation":"cwr.test","input":{{}}}}'
exec 0<&-
i=1
while [ "$i" -le 12 ]; do
  printf '{{"type":"progress","invocation_ref":"{INVOCATION_REF}","sequence":%s,"status":102,"summary":"progress %s"}}\n' "$i" "$i"
  i=$((i + 1))
done
printf '%s\n' '{{"type":"result","invocation_ref":"{INVOCATION_REF}","sequence":13,"status":504,"summary":"Command timed out.","error":{{"code":"command_timeout"}}}}'
sleep 0.5
"##,
    ));
    let adapter = adapter(&executable, &directory.path().join("unused"));
    let mut progress = Vec::new();
    let outcome = run_process_adapter_with_controlled_host_calls(
        &adapter,
        invocation(true),
        &ProcessAdapterStopControl::new(),
        |event| progress.push(event.summary),
        |_| {
            thread::sleep(Duration::from_millis(200));
            Ok(AdapterHostCallOutcome::Returned(json!({})))
        },
    )
    .expect("progress backpressure must not hide an observed terminal");

    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter was unexpectedly stopped")
    };
    assert_eq!(progress.len(), 4);
    assert_eq!(terminal.sequence, 13);
    assert_eq!(terminal.status, 504);
    assert_eq!(terminal.error.as_ref().unwrap().code, "command_timeout");
}

#[test]
fn broken_host_reply_pipe_preserves_progress_before_a_queued_terminal() {
    let (directory, executable) = fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"call-1","operation":"cwr.test","input":{{}}}}'
exec 0<&-
printf '%s\n' '{{"type":"progress","invocation_ref":"{INVOCATION_REF}","sequence":1,"status":102,"summary":"finishing"}}'
printf '%s\n' '{{"type":"result","invocation_ref":"{INVOCATION_REF}","sequence":2,"status":504,"summary":"Command timed out.","error":{{"code":"command_timeout"}}}}'
sleep 0.5
"##,
    ));
    let adapter = adapter(&executable, &directory.path().join("unused"));
    let mut progress = Vec::new();
    let outcome = run_process_adapter_with_controlled_host_calls(
        &adapter,
        invocation(true),
        &ProcessAdapterStopControl::new(),
        |event| progress.push(event.summary),
        |_| {
            thread::sleep(Duration::from_millis(200));
            Ok(AdapterHostCallOutcome::Returned(json!({})))
        },
    )
    .expect("queued progress must not hide the terminal after a broken host reply pipe");

    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter was unexpectedly stopped")
    };
    assert_eq!(progress, ["finishing"]);
    assert_eq!(terminal.status, 504);
    assert_eq!(terminal.error.as_ref().unwrap().code, "command_timeout");
}

#[test]
fn repeated_broken_host_replies_preserve_a_terminal_already_in_the_pending_queue() {
    let (directory, executable) = fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"call-1","operation":"cwr.test","input":{{}}}}'
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"call-2","operation":"cwr.test","input":{{}}}}'
printf '%s\n' '{{"type":"result","invocation_ref":"{INVOCATION_REF}","sequence":1,"status":504,"summary":"Command timed out.","error":{{"code":"command_timeout"}}}}'
exec 0<&-
sleep 0.5
"##,
    ));
    let adapter = adapter(&executable, &directory.path().join("unused"));
    let outcome = run_process_adapter_with_controlled_host_calls(
        &adapter,
        invocation(true),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |_| {
            thread::sleep(Duration::from_millis(200));
            Ok(AdapterHostCallOutcome::Returned(json!({})))
        },
    )
    .expect("a pending terminal must survive every broken host reply");

    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter was unexpectedly stopped")
    };
    assert_eq!(terminal.status, 504);
    assert_eq!(terminal.error.as_ref().unwrap().code, "command_timeout");
}
