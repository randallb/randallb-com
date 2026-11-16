use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use cwr::adapter::{
    run_process_adapter, run_process_adapter_with_controlled_host_calls, AdapterHostCallOutcome,
    AdapterHostOperation, AdapterInvocation, ProcessAdapterFailure, ProcessAdapterOutcome,
    ProcessAdapterStopControl, ResolvedProcessAdapter, MAX_ACTION_VALUE_BYTES,
};
use serde_json::{json, Value};
use tempfile::TempDir;

const INVOCATION_REF: &str = "cwp://org.coworkerprotocol/2026-09/invocations/process-adapter-test";
const FIXTURE_READY_TIMEOUT: Duration = Duration::from_secs(5);

#[test]
fn failed_adapter_retains_exit_status_and_capped_stderr() {
    for stderr in ["provider failed".to_owned(), "x".repeat(150 * 1024)] {
        let (directory, executable) = fixture(&format!(
            "#!/bin/sh\nIFS= read -r invoke\nprintf '%s' '{stderr}' >&2\nexit 23\n"
        ));
        let adapter = adapter(&executable, &directory.path().join("unused"));
        let error = run_process_adapter(
            &adapter,
            invocation(false),
            &ProcessAdapterStopControl::new(),
            |_| {},
        )
        .expect_err("an adapter that exits without a result must fail");
        let failure = error
            .downcast_ref::<ProcessAdapterFailure>()
            .expect("process failure must retain diagnostics");

        assert_eq!(failure.exit_code, Some(23));
        assert_eq!(failure.signal, None);
        assert!(failure.message.contains("before returning a result"));
        assert_eq!(failure.stderr_total_bytes, stderr.len() as u64);
        assert_eq!(
            failure.stderr_tail,
            stderr[stderr.len().saturating_sub(128 * 1024)..]
        );
        assert_eq!(failure.stderr_truncated, stderr.len() > 128 * 1024);
        assert!(!error.to_string().contains(&stderr));
    }
}

#[test]
fn failed_adapter_returns_promptly_when_an_escaped_process_keeps_stderr_open() {
    // Bash supports job control without a TTY; Linux /bin/sh may disable it.
    let (directory, executable) = fixture(
        r##"#!/usr/bin/env bash
IFS= read -r invoke
printf 'diagnostic marker' >&2
set -m
/bin/sleep 30 >/dev/null &
printf '%s\n' "$!" > "$2"
: > "$1"
exit 23
"##,
    );
    let adapter_ready = directory.path().join("adapter-ready");
    let output_holder_pid = directory.path().join("output-holder-pid");
    let mut output_holder_cleanup = ProcessCleanup::new(output_holder_pid.clone());
    let adapter = adapter(&executable, &adapter_ready).with_argument(&output_holder_pid);
    let (runner_finished, wait_for_runner) = mpsc::channel();
    let runner = thread::spawn(move || {
        let outcome = run_process_adapter(
            &adapter,
            invocation(false),
            &ProcessAdapterStopControl::new(),
            |_| {},
        );
        runner_finished.send(()).expect("signal adapter completion");
        outcome
    });
    wait_for_marker(&adapter_ready);
    wait_for_pid(&output_holder_pid);
    let prompt_completion = wait_for_runner.recv_timeout(Duration::from_millis(750));

    output_holder_cleanup.kill();
    let error = runner
        .join()
        .expect("adapter runner must not panic")
        .expect_err("an adapter that exits without a result must fail");
    assert!(
        prompt_completion.is_ok(),
        "diagnostics waited for an escaped process"
    );
    let failure = error.downcast_ref::<ProcessAdapterFailure>().unwrap();
    assert_eq!(failure.exit_code, Some(23));
    assert_eq!(failure.stderr_tail, "diagnostic marker");
    assert_eq!(failure.stderr_total_bytes, 17);
    assert!(!failure.stderr_complete);
}

#[test]
fn stop_before_launch_does_not_start_the_adapter() {
    let (directory, executable) = fixture("#!/bin/sh\n: > \"$1\"\n");
    let marker = directory.path().join("started");
    let adapter = adapter(&executable, &marker);
    let stop = ProcessAdapterStopControl::new();
    stop.stop();

    let outcome = run_process_adapter(&adapter, invocation(false), &stop, |_| {})
        .expect("stop before adapter launch");

    assert_eq!(outcome, ProcessAdapterOutcome::Stopped);
    assert!(!marker.exists(), "a stopped adapter must not be launched");
}

#[test]
fn stop_after_a_host_handler_fences_the_call_result() {
    let (directory, executable) = host_call_fixture();
    let marker = directory.path().join("call-result-received");
    let adapter = adapter(&executable, &marker);
    let stop = ProcessAdapterStopControl::new();
    let stop_for_runner = stop.clone();
    let (handler_entered, wait_for_handler) = mpsc::channel();
    let (release_handler, handler_release) = mpsc::channel();

    let runner = thread::spawn(move || {
        run_process_adapter_with_controlled_host_calls(
            &adapter,
            invocation(true),
            &stop_for_runner,
            |_| {},
            move |_| {
                handler_entered.send(()).expect("signal host handler");
                handler_release.recv().expect("release host handler");
                Ok(AdapterHostCallOutcome::Returned(json!({
                    "payload": "x".repeat(2 * 1024 * 1024)
                })))
            },
        )
    });
    wait_for_handler.recv().expect("host handler must start");
    stop.stop();
    release_handler.send(()).expect("release host handler");
    let outcome = runner
        .join()
        .expect("adapter runner must not panic")
        .expect("stop after host handler");

    assert_eq!(outcome, ProcessAdapterOutcome::Stopped);
    assert!(
        !marker.exists(),
        "the adapter must not receive a CallResult after stop is requested"
    );
}

#[test]
fn stop_after_result_while_waiting_for_process_exit_is_stopped() {
    let (directory, executable) = result_then_delayed_exit_fixture();
    let result_emitted = directory.path().join("result-emitted");
    let adapter = adapter(&executable, &result_emitted);
    let stop = ProcessAdapterStopControl::new();
    let runner_stop = stop.clone();

    let runner = thread::spawn(move || {
        run_process_adapter(&adapter, invocation(false), &runner_stop, |_| {})
    });
    wait_for_marker(&result_emitted);
    // The adapter has flushed its valid Result and deliberately remains alive.
    // Give the protocol reader several poll intervals to put the runner inside
    // its process-exit wait before issuing the concurrent stop.
    thread::sleep(Duration::from_millis(200));
    stop.stop();

    let outcome = runner
        .join()
        .expect("adapter runner must not panic")
        .expect("stop after a valid Result is not a protocol failure");
    assert_eq!(outcome, ProcessAdapterOutcome::Stopped);
}

#[test]
fn stop_after_silent_adapter_exit_while_output_remains_open_is_stopped() {
    let (directory, executable) = silent_adapter_with_output_holder_fixture();
    let adapter_waiting = directory.path().join("adapter-waiting");
    let adapter = adapter(&executable, &adapter_waiting);
    let stop = ProcessAdapterStopControl::new();
    let runner_stop = stop.clone();

    let runner = thread::spawn(move || {
        run_process_adapter(&adapter, invocation(false), &runner_stop, |_| {})
    });
    wait_for_marker(&adapter_waiting);
    // The detached background process keeps stdout connected after stop kills
    // the adapter itself, so the runner observes the exit from its timeout
    // branch rather than through a protocol-reader disconnect.
    thread::sleep(Duration::from_millis(200));
    stop.stop();

    let outcome = runner
        .join()
        .expect("adapter runner must not panic")
        .expect("stop during a silent adapter exit is not a protocol failure");
    assert_eq!(outcome, ProcessAdapterOutcome::Stopped);
}

#[test]
fn stop_returns_promptly_when_an_escaped_process_keeps_output_open() {
    let (directory, executable) = silent_adapter_with_long_lived_output_holder_fixture();
    let adapter_waiting = directory.path().join("adapter-waiting");
    let output_holder_pid = directory.path().join("output-holder-pid");
    let mut output_holder_cleanup = ProcessCleanup::new(output_holder_pid.clone());
    let adapter = adapter(&executable, &adapter_waiting).with_argument(&output_holder_pid);
    let stop = ProcessAdapterStopControl::new();
    let runner_stop = stop.clone();
    let (runner_finished, wait_for_runner) = mpsc::channel();

    let runner = thread::spawn(move || {
        let outcome = run_process_adapter(&adapter, invocation(false), &runner_stop, |_| {});
        runner_finished
            .send(())
            .expect("signal process adapter completion");
        outcome
    });
    wait_for_marker(&adapter_waiting);
    wait_for_pid(&output_holder_pid);
    thread::sleep(Duration::from_millis(200));

    let stopped_at = Instant::now();
    stop.stop();
    let prompt_completion = wait_for_runner.recv_timeout(Duration::from_millis(750));
    let stop_elapsed = stopped_at.elapsed();

    // Always release the escaped process before joining the adapter thread so
    // a failing regression neither stalls the suite nor leaves a stray child.
    output_holder_cleanup.kill();
    let outcome = runner
        .join()
        .expect("adapter runner must not panic")
        .expect("stop with an escaped output holder");

    assert!(
        prompt_completion.is_ok(),
        "stop waited for an unrelated process to close adapter stdout"
    );
    assert!(
        stop_elapsed < Duration::from_millis(750),
        "stop must return within its prompt-return bound"
    );
    assert_eq!(outcome, ProcessAdapterOutcome::Stopped);
}

#[test]
fn completed_result_returns_promptly_when_an_escaped_process_keeps_output_open() {
    let (directory, executable) = result_with_long_lived_output_holder_fixture();
    let result_emitted = directory.path().join("result-emitted");
    let output_holder_pid = directory.path().join("output-holder-pid");
    let mut output_holder_cleanup = ProcessCleanup::new(output_holder_pid.clone());
    let adapter = adapter(&executable, &result_emitted).with_argument(&output_holder_pid);
    let (runner_finished, wait_for_runner) = mpsc::channel();

    let runner = thread::spawn(move || {
        let outcome = run_process_adapter(
            &adapter,
            invocation(false),
            &ProcessAdapterStopControl::new(),
            |_| {},
        );
        runner_finished
            .send(())
            .expect("signal process adapter completion");
        outcome
    });
    wait_for_marker(&result_emitted);
    wait_for_pid(&output_holder_pid);

    let completed_at = Instant::now();
    let prompt_completion = wait_for_runner.recv_timeout(Duration::from_millis(750));
    let completion_elapsed = completed_at.elapsed();

    // Release the escaped holder before joining even when this assertion is
    // red, so the regression cannot stall or leave a stray child process.
    output_holder_cleanup.kill();
    let outcome = runner
        .join()
        .expect("adapter runner must not panic")
        .expect("valid result with an escaped output holder");

    assert!(
        prompt_completion.is_ok(),
        "completion waited for an unrelated process to close adapter stdout"
    );
    assert!(
        completion_elapsed < Duration::from_millis(750),
        "completion must return within its prompt-return bound"
    );
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
}

#[test]
fn stop_after_silent_output_disconnect_while_waiting_for_exit_is_stopped() {
    let (directory, executable) = silent_output_disconnect_fixture();
    let output_closed = directory.path().join("output-closed");
    let adapter = adapter(&executable, &output_closed);
    let stop = ProcessAdapterStopControl::new();
    let runner_stop = stop.clone();

    let runner = thread::spawn(move || {
        run_process_adapter(&adapter, invocation(false), &runner_stop, |_| {})
    });
    wait_for_marker(&output_closed);
    // Closing stdout disconnects the protocol reader while the adapter stays
    // alive, putting the runner inside its process-exit wait before stop.
    thread::sleep(Duration::from_millis(200));
    stop.stop();

    let outcome = runner
        .join()
        .expect("adapter runner must not panic")
        .expect("stop after a silent output disconnect is not a protocol failure");
    assert_eq!(outcome, ProcessAdapterOutcome::Stopped);
}

fn invocation(with_host_call: bool) -> AdapterInvocation {
    let mut invocation = AdapterInvocation::new(
        INVOCATION_REF,
        "test--action",
        json!({}),
        json!({"type": "object"}),
    );
    if with_host_call {
        invocation.host_operations.push(AdapterHostOperation {
            name: "cwr.test".into(),
            description: "Exercise the controlled host-call boundary.".into(),
            input_schema: json!({"type": "object"}),
            output_schema: json!({"type": "object"}),
        });
    }
    invocation
}

fn host_call_fixture() -> (TempDir, PathBuf) {
    fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"call-1","operation":"cwr.test","input":{{}}}}'
if /bin/dd bs=1 count=1 of=/dev/null 2>/dev/null; then
  : > "$1"
  /bin/cat >/dev/null
fi
"##
    ))
}

fn result_then_delayed_exit_fixture() -> (TempDir, PathBuf) {
    fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
printf '%s\n' '{{"type":"result","invocation_ref":"{INVOCATION_REF}","sequence":1,"status":200,"summary":"Finished","result":{{}}}}'
: > "$1"
/bin/sleep 10
"##
    ))
}

fn silent_adapter_with_output_holder_fixture() -> (TempDir, PathBuf) {
    fixture(
        r##"#!/bin/sh
IFS= read -r invoke
printf 'diagnostic marker' >&2
set -m
/bin/sleep 1 &
: > "$1"
exec /bin/sleep 10
"##,
    )
}

fn silent_adapter_with_long_lived_output_holder_fixture() -> (TempDir, PathBuf) {
    fixture(
        r##"#!/bin/sh
IFS= read -r invoke
printf 'diagnostic marker' >&2
set -m
/bin/sleep 30 &
printf '%s\n' "$!" > "$2"
printf '{{'
: > "$1"
exec /bin/sleep 30
"##,
    )
}

fn result_with_long_lived_output_holder_fixture() -> (TempDir, PathBuf) {
    fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
printf 'diagnostic marker' >&2
set -m
/bin/sleep 30 &
printf '%s\n' "$!" > "$2"
printf '%s\n' '{{"type":"result","invocation_ref":"{INVOCATION_REF}","sequence":1,"status":200,"summary":"Finished","result":{{}}}}'
: > "$1"
"##
    ))
}

fn silent_output_disconnect_fixture() -> (TempDir, PathBuf) {
    fixture(
        r##"#!/bin/sh
IFS= read -r invoke
exec 1>&-
: > "$1"
exec /bin/sleep 10
"##,
    )
}

fn wait_for_marker(marker: &Path) {
    let deadline = Instant::now() + FIXTURE_READY_TIMEOUT;
    while !marker.exists() {
        assert!(
            Instant::now() < deadline,
            "adapter did not create its synchronization marker"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_pid(marker: &Path) {
    let deadline = Instant::now() + FIXTURE_READY_TIMEOUT;
    loop {
        if fs::read_to_string(marker)
            .ok()
            .and_then(|contents| contents.trim().parse::<i32>().ok())
            .is_some()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "adapter did not publish its child process id"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn fixture(script: &str) -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("create process adapter fixture");
    let executable = directory.path().join("adapter");
    fs::write(&executable, script).expect("write process adapter fixture");
    let mut permissions = fs::metadata(&executable)
        .expect("read process adapter fixture metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).expect("make process adapter fixture executable");
    (directory, executable)
}

fn adapter(executable: &Path, marker: &Path) -> ResolvedProcessAdapter {
    ResolvedProcessAdapter::new(executable).with_argument(marker)
}

struct ProcessCleanup {
    pid_path: PathBuf,
    killed: bool,
}

impl ProcessCleanup {
    fn new(pid_path: PathBuf) -> Self {
        Self {
            pid_path,
            killed: false,
        }
    }

    fn kill(&mut self) {
        if self.killed {
            return;
        }
        self.killed = true;
        let Ok(contents) = fs::read_to_string(&self.pid_path) else {
            return;
        };
        let Ok(pid) = contents.trim().parse::<i32>() else {
            return;
        };
        if pid > 0 {
            // SAFETY: the positive pid was written by the test child, and `kill` neither
            // dereferences pointers nor retains Rust state.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

impl Drop for ProcessCleanup {
    fn drop(&mut self) {
        self.kill();
    }
}

#[path = "process_adapter/broken_pipe.rs"]
mod broken_pipe;
#[path = "process_adapter/context.rs"]
mod context;
