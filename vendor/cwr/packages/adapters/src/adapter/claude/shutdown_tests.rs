use super::*;

#[test]
fn an_unread_large_tool_reply_cannot_block_cleanup() {
    let fixture = Fixture::new();
    let stop = ProcessAdapterStopControl::new();
    let timeout_stop = stop.clone();
    // Bound a regression in the test itself so a blocked write cannot hang CI.
    let (armed, begin) = std::sync::mpsc::channel();
    let (finished, wait) = std::sync::mpsc::channel();
    let watchdog = thread::spawn(move || {
        // Startup can be slow while other test providers launch. Measure the
        // blocked reply path only after the host has received the tool call.
        match begin.recv_timeout(Duration::from_secs(30)) {
            Ok(()) => {
                if wait.recv_timeout(Duration::from_secs(5)).is_err() {
                    timeout_stop.stop();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => timeout_stop.stop(),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {}
        }
    });
    let outcome = run_process_adapter_with_activity_and_host_calls(
        &fixture.adapter(),
        fixture.invocation("unread-reply"),
        &stop,
        |_| {},
        |_| Ok(()),
        |_| {
            armed.send(()).unwrap();
            Ok(json!({"text":"x".repeat(900_000)}))
        },
    );
    drop(armed);
    let _ = finished.send(());
    watchdog.join().unwrap();
    assert!(
        outcome.is_err(),
        "a stalled MCP reader must fail before the watchdog stops the invocation"
    );
    assert_children_stopped(fixture.directory.path());
}

#[test]
fn host_escalates_even_while_the_call_handler_is_blocked() {
    let fixture = Fixture::new();
    fs::write(&fixture.executable, r##"#!/bin/sh
trap '' TERM
read -r invocation
printf '%s\n' "$$" > provider.pid
printf '%s\n' '{"type":"call","invocation_ref":"cwp://com.example/invocations/claude-test","call_id":"one","operation":"cwp.test--probe--value","input":{"value":7}}'
exec /bin/sleep 30
"##).unwrap();
    let adapter = ResolvedProcessAdapter::new(&fixture.executable)
        .with_current_directory(fixture.directory.path())
        .with_shutdown_grace_period(Duration::from_millis(100));
    let stop = ProcessAdapterStopControl::new();
    let outcome = run_process_adapter_with_activity_and_host_calls(
        &adapter,
        fixture.invocation("unused"),
        &stop,
        |_| {},
        |_| Ok(()),
        |_| {
            stop.stop();
            // The runtime cannot return to its receive loop until this callback
            // completes. Its independent escalation timer still has to kill sleep.
            assert_children_stopped(fixture.directory.path());
            Ok(json!({}))
        },
    )
    .unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Stopped));
}

#[test]
fn host_allows_cooperative_shutdown_before_forcing_cleanup() {
    let fixture = Fixture::new();
    fs::write(&fixture.executable, r##"#!/bin/sh
trap 'printf cleaned > cleaned; exit 0' TERM
read -r invocation
printf '%s\n' "$$" > provider.pid
printf '%s\n' '{"type":"progress","invocation_ref":"cwp://com.example/invocations/claude-test","sequence":1,"status":100,"summary":"Ready"}'
while :; do /bin/sleep 0.01; done
"##).unwrap();
    let adapter = ResolvedProcessAdapter::new(&fixture.executable)
        .with_current_directory(fixture.directory.path())
        .with_shutdown_grace_period(Duration::from_secs(1));
    let stop = ProcessAdapterStopControl::new();
    let outcome = run_process_adapter(&adapter, fixture.invocation("unused"), &stop, |_| {
        stop.stop()
    })
    .unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Stopped));
    assert_eq!(
        fs::read_to_string(fixture.directory.path().join("cleaned")).unwrap(),
        "cleaned"
    );
    assert_children_stopped(fixture.directory.path());
}
