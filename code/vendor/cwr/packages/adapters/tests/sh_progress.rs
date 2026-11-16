use cwr::{
    action_contract::{invoke_action_input_schema, invoke_action_output_schema},
    adapter::{
        run_process_adapter, run_process_adapter_with_host_calls, AdapterHostOperation,
        AdapterInvocation, AdapterProgress, ProcessAdapterOutcome, ProcessAdapterStopControl,
        ResolvedProcessAdapter,
    },
};
use serde_json::json;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::Duration,
};

fn fixture(code: &str) -> (tempfile::TempDir, ResolvedProcessAdapter, AdapterInvocation) {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let helper = workspace.join("scripts/action_progress.ts");
    fs::write(
        directory.path().join("fixture.ts"),
        format!(
            "import {{ ActionProgressReporter, runWithProgress }} from {};\n{code}",
            serde_json::to_string(&format!("file://{}", helper.display())).unwrap(),
        ),
    )
    .unwrap();
    let adapter = ResolvedProcessAdapter::new(env!("CARGO_BIN_EXE_cwr-adapter-sh"))
        .with_current_directory(directory.path())
        .with_shutdown_grace_period(Duration::from_secs(2))
        .with_environment("PATH", std::env::var_os("PATH").unwrap());
    let mut invocation = AdapterInvocation::new(
        "cwp://example.test/invocations/progress",
        "test--progress",
        json!({"input":"preserved"}),
        json!({"type":"object"}),
    );
    invocation.actor_integration_params = json!({
        "command":"deno run --no-config --allow-run --allow-read --allow-write fixture.ts",
        "progress_jsonl_stderr_actions":["test--progress"], "fail_on_nonzero":true,
    });
    (directory, adapter, invocation)
}

fn run_fixture(
    adapter: &ResolvedProcessAdapter,
    invocation: AdapterInvocation,
    stop: &ProcessAdapterStopControl,
    on_progress: impl FnMut(AdapterProgress),
) -> ProcessAdapterOutcome {
    let timeout_stop = stop.clone();
    let (finished, wait) = mpsc::channel::<()>();
    let timeout = thread::spawn(move || {
        if matches!(
            wait.recv_timeout(Duration::from_secs(15)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            timeout_stop.stop();
        }
    });
    let outcome = run_process_adapter(adapter, invocation, stop, on_progress);
    drop(finished);
    timeout.join().unwrap();
    outcome.unwrap()
}

#[test]
fn helper_publishes_before_exit_and_preserves_success_and_failure_io() {
    for exit_code in [0, 7] {
        let (directory, adapter, invocation) = fixture(&format!(
            r#"
const reporter = new ActionProgressReporter("Fixture tests", "tests", 50);
reporter.emit("Tests: 1/2 complete.", {{key:"tests", phase:"testing", completed:1, total:2}});
const code = await runWithProgress("sh", ["-c",
  "read -r input; printf '%s' \"$input\"; printf diagnostic >&2; sleep 0.5; touch complete; exit {exit_code}"
], reporter);
// Remaining alive briefly must not keep the helper's timer running.
await new Promise(resolve => setTimeout(resolve, 100));
Deno.exit(code);
"#
        ));
        let mut records = Vec::new();
        let mut live_fallback = false;
        let outcome = run_fixture(
            &adapter,
            invocation,
            &ProcessAdapterStopControl::new(),
            |record| {
                if record.summary.contains("have not finished")
                    && !directory.path().join("complete").exists()
                {
                    live_fallback = true;
                }
                records.push(record);
            },
        );
        assert!(
            live_fallback,
            "fallback must arrive while command is still running"
        );
        assert!(!records[0].published, "adapter startup stays diagnostic");
        for (index, record) in records.iter().enumerate() {
            assert_eq!(record.sequence, index as u64 + 1);
            if index > 0 {
                assert!(record.published);
                assert_eq!(record.progress.as_ref().unwrap().completed, Some(1));
            }
        }
        let ProcessAdapterOutcome::Completed(result) = outcome else {
            panic!("fixture stopped")
        };
        assert_eq!(result.sequence, records.len() as u64 + 1);
        assert_eq!(result.status, if exit_code == 0 { 200 } else { 500 });
        let output = result
            .result
            .as_ref()
            .or_else(|| result.error.as_ref().unwrap().details.as_ref())
            .unwrap();
        assert_eq!(output["exit_code"], exit_code);
        assert_eq!(output["stdout"], "{\"input\":\"preserved\"}");
        assert_eq!(output["stderr"], "diagnostic");
    }
}

#[test]
fn stop_ends_helper_and_child_without_an_adapter_terminal_response() {
    let (directory, adapter, invocation) = fixture(
        r#"
Deno.writeTextFileSync("helper.pid", String(Deno.pid));
const reporter = new ActionProgressReporter("Fixture tests", "tests", 100);
await runWithProgress("sh", ["-c", "echo $$ > child.pid; exec sleep 30"], reporter);
"#,
    );
    let stop = ProcessAdapterStopControl::new();
    let mut saw_fallback = false;
    let outcome = run_fixture(&adapter, invocation, &stop, |record| {
        assert_eq!(record.status, 102);
        if record.published {
            assert!(record.summary.contains("have not finished"));
            saw_fallback = true;
            stop.stop();
        }
    });
    assert!(saw_fallback);
    assert!(matches!(outcome, ProcessAdapterOutcome::Stopped));
    for file in ["helper.pid", "child.pid"] {
        let pid = fs::read_to_string(directory.path().join(file)).unwrap();
        // A killed orphan can briefly remain a zombie until the OS reaps it.
        let mut running = true;
        for _ in 0..100 {
            let output = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", pid.trim()])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&output.stdout);
            running = !state.trim().is_empty() && !state.trim().starts_with('Z');
            if !running {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!running, "{file} survived cancellation");
    }
}

#[test]
fn stop_reaches_a_silent_shell_command_before_forced_adapter_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let adapter = ResolvedProcessAdapter::new(env!("CARGO_BIN_EXE_cwr-adapter-sh"))
        .with_current_directory(directory.path())
        .with_shutdown_grace_period(Duration::from_secs(2))
        .with_environment("PATH", std::env::var_os("PATH").unwrap());
    let mut invocation = AdapterInvocation::new(
        "cwp://example.test/invocations/silent-stop",
        "test--silent-stop",
        json!({}),
        json!({"type":"object"}),
    );
    invocation.actor_integration_params = json!({
        "command":"echo $$ > child.pid; exec sleep 30"
    });
    let stop = ProcessAdapterStopControl::new();
    let execution_stop = stop.clone();
    let execution = thread::spawn(move || {
        run_process_adapter(&adapter, invocation, &execution_stop, |_| {}).unwrap()
    });
    let pid_path = directory.path().join("child.pid");
    for _ in 0..200 {
        if pid_path.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(pid_path.exists(), "silent command did not start");
    stop.stop();
    assert!(matches!(
        execution.join().unwrap(),
        ProcessAdapterOutcome::Stopped
    ));

    let pid = fs::read_to_string(pid_path).unwrap();
    for _ in 0..100 {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid.trim()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout);
        if state.trim().is_empty() || state.trim().starts_with('Z') {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("silent shell command survived cancellation");
}

#[test]
fn nested_coworker_invocation_uses_the_shared_host_contract() {
    let directory = tempfile::tempdir().unwrap();
    let adapter = ResolvedProcessAdapter::new(env!("CARGO_BIN_EXE_cwr-adapter-sh"))
        .with_current_directory(directory.path())
        .with_shutdown_grace_period(Duration::from_secs(2))
        .with_environment("PATH", std::env::var_os("PATH").unwrap());
    let mut invocation = AdapterInvocation::new(
        "cwp://example.test/invocations/nested-coworker",
        "test--nested-coworker",
        json!({}),
        json!({"type":"object"}),
    );
    invocation.actor_integration_params = json!({
        "command":"printf '%s' \"$CWR_COWORKER_INVOKE_SOCKET\" > socket-path.tmp && mv socket-path.tmp socket-path; while [ ! -f done ]; do sleep 0.01; done",
        "timeout_ms":5_000
    });
    invocation.host_operations = vec![
        AdapterHostOperation {
            name: "cwr.list_actions".into(),
            description: "List actions for the shell coworker bridge.".into(),
            input_schema: json!({"type":"object", "additionalProperties":false}),
            output_schema: json!({"type":"object"}),
        },
        AdapterHostOperation {
            name: "cwr.invoke_action".into(),
            description: "Invoke an exact discovered action target.".into(),
            input_schema: invoke_action_input_schema(),
            output_schema: invoke_action_output_schema(),
        },
    ];
    let target = json!({
        "runtime_ref":"cwp://example.test/runtimes/local",
        "coworker_ref":"cwp://example.test/coworkers/helper",
        "role_ref":"cwp://example.test/roles/helper",
        "action_name":"run--task",
        "binding_ref":"cwp://example.test/bindings/helper",
        "contract_id":"sha256:test"
    });
    let socket_path = directory.path().join("socket-path");
    let done = directory.path().join("done");
    let client_target = target.clone();
    let client = thread::spawn(move || {
        for _ in 0..200 {
            if let Ok(path) = fs::read_to_string(&socket_path) {
                let mut stream = UnixStream::connect(path).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                writeln!(
                    stream,
                    "{}",
                    json!({"type":"describe_action", "activation":"@helper--run--task"})
                )
                .unwrap();
                let mut response = String::new();
                reader.read_line(&mut response).unwrap();
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&response).unwrap()["type"],
                    "action"
                );
                writeln!(
                    stream,
                    "{}",
                    json!({
                        "type":"invoke_action",
                        "activation":"@helper--run--task",
                        "input":{"count":3}
                    })
                )
                .unwrap();
                response.clear();
                reader.read_line(&mut response).unwrap();
                let response: serde_json::Value = serde_json::from_str(&response).unwrap();
                assert_eq!(response["type"], "completed");
                assert_eq!(response["result"]["result"], json!({"ok":true}));
                fs::write(done, "").unwrap();
                return client_target;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("shell adapter did not publish its coworker socket");
    });

    let expected_target = target.clone();
    let outcome = run_process_adapter_with_host_calls(
        &adapter,
        invocation,
        &ProcessAdapterStopControl::new(),
        |_| {},
        move |call| match call.operation.as_str() {
            "cwr.list_actions" => Ok(json!({"actions":[{
                "activation":"@helper--run--task",
                "is_default_binding":true,
                "input_schema":{"type":"object"},
                "target":expected_target
            }]})),
            "cwr.invoke_action" => {
                assert_eq!(call.input["target"], expected_target);
                assert_eq!(call.input["input"], json!({"count":3}));
                assert!(call.input["timeout_ms"]
                    .as_u64()
                    .is_some_and(|timeout| timeout <= 5_000));
                Ok(json!({
                    "invocation_ref":"cwp://example.test/invocations/child",
                    "sequence":1,
                    "status":200,
                    "summary":"Finished.",
                    "result":{"ok":true}
                }))
            }
            operation => panic!("unexpected host operation {operation}"),
        },
    )
    .unwrap();
    assert_eq!(client.join().unwrap(), target);
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(result) if result.status == 200));
}

#[test]
fn delayed_nested_host_reply_preserves_the_shell_timeout_terminal() {
    let directory = tempfile::tempdir().unwrap();
    let adapter = ResolvedProcessAdapter::new(env!("CARGO_BIN_EXE_cwr-adapter-sh"))
        .with_current_directory(directory.path())
        .with_shutdown_grace_period(Duration::from_secs(2))
        .with_environment("PATH", std::env::var_os("PATH").unwrap());
    let mut invocation = AdapterInvocation::new(
        "cwp://example.test/invocations/nested-timeout",
        "test--nested-timeout",
        json!({}),
        json!({"type":"object"}),
    );
    invocation.actor_integration_params = json!({
        "command":"printf partial; printf '%s' \"$CWR_COWORKER_INVOKE_SOCKET\" > socket-path.tmp && mv socket-path.tmp socket-path; sleep 30",
        "timeout_ms":500
    });
    invocation.host_operations = vec![
        AdapterHostOperation {
            name: "cwr.list_actions".into(),
            description: "List actions for the shell coworker bridge.".into(),
            input_schema: json!({"type":"object", "additionalProperties":false}),
            output_schema: json!({"type":"object"}),
        },
        AdapterHostOperation {
            name: "cwr.invoke_action".into(),
            description: "Invoke an exact discovered action target.".into(),
            input_schema: invoke_action_input_schema(),
            output_schema: invoke_action_output_schema(),
        },
    ];
    let target = json!({
        "runtime_ref":"cwp://example.test/runtimes/local",
        "coworker_ref":"cwp://example.test/coworkers/helper",
        "role_ref":"cwp://example.test/roles/helper",
        "action_name":"run--task",
        "binding_ref":"cwp://example.test/bindings/helper",
        "contract_id":"sha256:test"
    });
    let socket_path = directory.path().join("socket-path");
    let client = thread::spawn(move || {
        for _ in 0..200 {
            if let Ok(path) = fs::read_to_string(&socket_path) {
                let mut stream = UnixStream::connect(path).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                writeln!(
                    stream,
                    "{}",
                    json!({"type":"describe_action", "activation":"@helper--run--task"})
                )
                .unwrap();
                let mut response = String::new();
                reader.read_line(&mut response).unwrap();
                writeln!(
                    stream,
                    "{}",
                    json!({"type":"invoke_action", "activation":"@helper--run--task", "input":{}})
                )
                .unwrap();
                response.clear();
                let _ = reader.read_line(&mut response);
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("shell adapter did not publish its coworker socket");
    });
    let expected_target = target.clone();
    let outcome = run_process_adapter_with_host_calls(
        &adapter,
        invocation,
        &ProcessAdapterStopControl::new(),
        |_| {},
        move |call| match call.operation.as_str() {
            "cwr.list_actions" => Ok(json!({"actions":[{
                "activation":"@helper--run--task",
                "is_default_binding":true,
                "input_schema":{"type":"object"},
                "target":expected_target
            }]})),
            "cwr.invoke_action" => {
                thread::sleep(Duration::from_millis(700));
                Ok(json!({
                    "invocation_ref":"cwp://example.test/invocations/child",
                    "sequence":1,
                    "status":200,
                    "summary":"Finished.",
                    "result":{}
                }))
            }
            operation => panic!("unexpected host operation {operation}"),
        },
    )
    .unwrap();
    client.join().unwrap();
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter was unexpectedly stopped")
    };
    assert_eq!(terminal.status, 504);
    let error = terminal.error.unwrap();
    assert_eq!(error.code, "command_timeout");
    assert_eq!(error.details.unwrap()["stdout"], "partial");
}
