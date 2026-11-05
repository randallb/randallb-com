use cwr::adapter::{
    run_process_adapter, AdapterEvent, AdapterHostMessage, AdapterInvocation,
    ProcessAdapterOutcome, ProcessAdapterStopControl, ResolvedProcessAdapter,
    PROCESS_ADAPTER_PROTOCOL_REF,
};
use serde_json::json;
use std::{
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[test]
fn shell_run_command_completes_after_streamed_context_delivery() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    // `read` blocks indefinitely if the command inherits the adapter's open
    // process-protocol stdin. Ordinary run--command actions receive closed
    // stdin, while configured actions can still opt into JSON input.
    let command = "read ignored || true; printf done";

    for context_bytes in [22 * 1024, 207 * 1024] {
        let adapter = ResolvedProcessAdapter::new(env!("CARGO_BIN_EXE_cwr-adapter-sh"))
            .with_current_directory(&workspace)
            .with_environment("PATH", std::env::var_os("PATH").unwrap());
        let mut invocation = AdapterInvocation::new(
            format!("cwp://example.test/invocations/context-{context_bytes}"),
            "run--command",
            json!({"command": command}),
            json!({"type": "object"}),
        );
        invocation.thread = json!({"messages": [{"text": "x".repeat(context_bytes)}]});
        let stop = ProcessAdapterStopControl::new();
        let timeout_stop = stop.clone();
        let (finished, wait) = mpsc::channel::<()>();
        let timeout = thread::spawn(move || {
            if wait.recv_timeout(Duration::from_secs(5)).is_err() {
                timeout_stop.stop();
            }
        });

        let outcome = run_process_adapter(&adapter, invocation, &stop, |_| {}).unwrap();
        drop(finished);
        timeout.join().unwrap();
        let ProcessAdapterOutcome::Completed(result) = outcome else {
            panic!("Shell action stalled after delivering {context_bytes} bytes of context");
        };
        assert_eq!(result.status, 200, "{result:?}");
        assert_eq!(result.result.unwrap()["stdout"], "done");
    }
}

#[test]
fn shell_drains_output_while_delivering_large_action_input() {
    let mut invocation = AdapterInvocation::new(
        "cwp://example.test/invocations/pipes",
        "run--pipes",
        json!({"text": "x".repeat(256 * 1024)}),
        json!({"type":"object"}),
    );
    let expected_input_bytes = serde_json::to_vec(&invocation.input).unwrap().len() + 1;
    invocation.actor_integration_params = json!({
        "command": "(dd if=/dev/zero bs=65536 count=32 2>/dev/null; printf '\\nstdout end 🦀\\n') & (dd if=/dev/zero bs=65536 count=32 2>/dev/null; printf '\\nstderr end café\\n') >&2 & wait; wc -c"
    });
    let events = run_shell_invocation(invocation);
    let Some(AdapterEvent::Result {
        status: 200,
        result: Some(result),
        ..
    }) = events.last()
    else {
        panic!("expected a successful terminal result");
    };
    let stdout = result["stdout"].as_str().unwrap();
    let stderr = result["stderr"].as_str().unwrap();
    assert_eq!(
        stdout
            .lines()
            .last()
            .unwrap()
            .trim()
            .parse::<usize>()
            .unwrap(),
        expected_input_bytes
    );
    assert!(stdout.contains("stdout end 🦀"));
    assert!(stderr.ends_with("stderr end café\n"));
    for output in [stdout, stderr] {
        assert!(output.starts_with("[earlier output truncated]\n"));
        assert!(output.len() <= 64 * 1024);
    }
}

#[test]
fn failed_shell_action_keeps_both_final_diagnostics_after_overflow() {
    let mut invocation = AdapterInvocation::new(
        "cwp://example.test/invocations/failure",
        "run--failure",
        json!({}),
        json!({}),
    );
    invocation.actor_integration_params = json!({
        "command": "(dd if=/dev/zero bs=65536 count=32 2>/dev/null; printf '\\nstdout failure 🦀\\n') & (dd if=/dev/zero bs=65536 count=32 2>/dev/null; printf '\\nstderr failure café\\n') >&2 & wait; exit 7",
        "fail_on_nonzero": true,
    });
    let events = run_shell_invocation(invocation);
    let Some(AdapterEvent::Result {
        status: 500,
        error: Some(error),
        ..
    }) = events.last()
    else {
        panic!("expected a failed terminal result");
    };
    assert_eq!(error.code, "command_failed");
    let result = error.details.as_ref().unwrap();
    assert_eq!(result["exit_code"], 7);
    assert!(result["stdout"]
        .as_str()
        .unwrap()
        .ends_with("stdout failure 🦀\n"));
    assert!(result["stderr"]
        .as_str()
        .unwrap()
        .ends_with("stderr failure café\n"));
    for stream in ["stdout", "stderr"] {
        let output = result[stream].as_str().unwrap();
        assert!(output.starts_with("[earlier output truncated]\n"));
        assert!(output.len() <= 64 * 1024);
    }
}

fn run_shell_invocation(invocation: AdapterInvocation) -> Vec<AdapterEvent> {
    let frame = serde_json::to_vec(&AdapterHostMessage::Invoke {
        protocol: PROCESS_ADAPTER_PROTOCOL_REF.into(),
        invocation: Box::new(invocation),
    })
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cwr-adapter-sh"))
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let input = thread::spawn(move || {
        stdin.write_all(&frame).unwrap();
        stdin.write_all(b"\n").unwrap();
    });
    let mut stdout = child.stdout.take().unwrap();
    let output = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            // SAFETY: this test owns the child's separate process group and
            // has not reaped its leader, so its ID cannot have been reused.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            child.wait().unwrap();
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    input.join().unwrap();
    let bytes = output.join().unwrap();
    assert!(
        status.is_some_and(|status| status.success()),
        "shell adapter blocked on its command's pipes"
    );
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
