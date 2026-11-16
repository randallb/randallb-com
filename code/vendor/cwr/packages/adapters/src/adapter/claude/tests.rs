use cwr::adapter::{
    run_process_adapter, run_process_adapter_with_activity_and_host_calls, AdapterHostOperation,
    AdapterInvocation, ProcessAdapterOutcome, ProcessAdapterStopControl, ResolvedProcessAdapter,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

mod fixture;
mod native_smoke;
mod presentation_tests;
mod regression_tests;
mod shutdown_tests;
mod working_tests;

const SESSION: &str = "ad8e4c31-98d4-4fbb-8b88-f1d002c00ca9";

struct Fixture {
    directory: tempfile::TempDir,
    executable: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("PROMPT.md"),
            "Use only the declared coworker actions.",
        )
        .unwrap();
        fs::create_dir(directory.path().join("home")).unwrap();
        let executable = directory.path().join("claude");
        let test = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .replace('\'', "'\\''");
        fs::write(&executable, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > arguments\nprintf '%s\\n' \"$PATH\" > provider-path\nexport CWR_CLAUDE_FIXTURE=1\nexec '{test}' --exact fixture::provider_process --nocapture 3>&1 1>/dev/null\n")).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            directory,
            executable,
        }
    }

    fn adapter(&self) -> ResolvedProcessAdapter {
        self.adapter_executable(Path::new(env!("CARGO_BIN_EXE_cwr-adapter-claude")))
    }

    fn adapter_executable(&self, executable: &Path) -> ResolvedProcessAdapter {
        ResolvedProcessAdapter::new(executable)
            .with_current_directory(self.directory.path())
            .with_environment("CWR_COWORKER_DIRECTORY", self.directory.path())
            .with_environment("HOME", self.directory.path().join("home"))
            .with_environment("PATH", "/bundled/codex-path:/usr/bin:/bin")
            .with_environment("CLAUDE_CONFIG_DIR", self.directory.path().join("home"))
            .with_environment(
                "CWR_CLAUDE_DANGER_FULL_ACCESS_WORKSPACE",
                self.directory.path(),
            )
            .with_environment("CWR_CLAUDE_TRACE", "1")
            .with_shutdown_grace_period(Duration::from_secs(2))
    }

    fn invocation(&self, scenario: &str) -> AdapterInvocation {
        let mut invocation = AdapterInvocation::new(
            "cwp://com.example/invocations/claude-test",
            "test--action",
            json!({"message":"please work"}),
            json!({"type":"object","required":["message"],"additionalProperties":false,"properties":{"message":{"type":"string"}}}),
        );
        invocation.thread = json!({"messages":[{"text":"hello"}]});
        invocation.adapter_params =
            json!({"entrypoint":"./PROMPT.md","model":scenario,"timeout_seconds":10});
        invocation.integration_params = json!({"claude_executable":self.executable});
        invocation.host_operations = vec![AdapterHostOperation {
            name: "cwp.test--probe--value".into(),
            description: "Exercise host tools".into(),
            input_schema: json!({"type":"object","required":["value"],"additionalProperties":false,"properties":{"value":{"type":"integer","minimum":1}}}),
            output_schema: json!({"type":"object"}),
        }];
        invocation
    }

    fn run(&self, scenario: &str) -> cwr::CwrResult<ProcessAdapterOutcome> {
        trace(format!("scenario={scenario} host run starting"));
        let outcome = run_process_adapter(
            &self.adapter(),
            self.invocation(scenario),
            &ProcessAdapterStopControl::new(),
            |_| {},
        );
        trace(format!(
            "scenario={scenario} host run finished outcome={}",
            if outcome.is_ok() { "ok" } else { "error" }
        ));
        outcome
    }
}

fn trace(event: impl std::fmt::Display) {
    let mut diagnostics = std::io::stderr().lock();
    let _ = writeln!(
        diagnostics,
        "cwr-claude-test: trace pid={} event={event}",
        std::process::id()
    );
}

#[test]
fn provider_inherits_the_host_tool_path() {
    let fixture = Fixture::new();
    fixture.run("success").unwrap();
    assert_eq!(
        fs::read_to_string(fixture.directory.path().join("provider-path")).unwrap(),
        "/bundled/codex-path:/usr/bin:/bin\n"
    );
}

#[test]
fn structured_result_rejects_session_resume_without_a_call_recovery_contract() {
    let fixture = Fixture::new();
    let ProcessAdapterOutcome::Completed(first) = fixture.run("success").unwrap() else {
        panic!("expected completed")
    };
    assert_eq!(first.result, Some(json!({"message":"completed"})));
    assert_eq!(first.continuation, Some(json!(SESSION)));
    let mut invocation = fixture.invocation("success");
    invocation.continuation = first.continuation;
    run_process_adapter(
        &fixture.adapter(),
        invocation,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .expect_err("session id alone cannot safely resume an interrupted action");
    let args = fs::read_to_string(fixture.directory.path().join("arguments")).unwrap();
    assert!(!args.contains("--resume"));
    assert!(args.contains("--tools\n\n"));
    assert!(args.contains("--permission-mode\ndontAsk\n"));
    assert!(!args.contains("--dangerously-skip-permissions"));
}

#[test]
fn workspace_authority_selects_native_repository_tools() {
    for (authority, expected_tools, bypasses_permissions) in [
        (vec![], Vec::<&str>::new(), false),
        (
            vec!["workspace.read".into()],
            vec!["Read", "Glob", "Grep"],
            false,
        ),
        (vec!["workspace.write".into()], vec!["Edit", "Write"], false),
        (vec!["process.execute".into()], vec!["Bash"], true),
        (
            vec![
                "workspace.read".into(),
                "workspace.write".into(),
                "process.execute".into(),
            ],
            vec!["Read", "Glob", "Grep", "Edit", "Write", "Bash"],
            true,
        ),
    ] {
        let fixture = Fixture::new();
        let mut invocation = fixture.invocation("success");
        invocation.authority = authority;
        let outcome = run_process_adapter(
            &fixture.adapter(),
            invocation,
            &ProcessAdapterStopControl::new(),
            |_| {},
        )
        .unwrap();
        assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
        let args = fs::read_to_string(fixture.directory.path().join("arguments")).unwrap();
        let args: Vec<_> = args.lines().collect();
        let tools = args
            .windows(2)
            .find(|pair| pair[0] == "--tools")
            .map(|pair| pair[1])
            .unwrap();
        assert_eq!(
            tools
                .split(',')
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>(),
            expected_tools
        );
        let allowed = args
            .windows(2)
            .find(|pair| pair[0] == "--allowedTools")
            .map(|pair| pair[1]);
        for tool in &expected_tools {
            assert!(allowed.is_some_and(|allowed| allowed.split(',').any(|name| name == *tool)));
        }
        assert_eq!(
            args.contains(&"--dangerously-skip-permissions"),
            bypasses_permissions
        );
        assert!(!tools
            .split(',')
            .any(|tool| tool.starts_with("Web") || tool == "Task"));
    }
}

#[test]
fn native_repository_tools_require_an_exact_host_workspace_grant() {
    let fixture = Fixture::new();
    let mut invocation = fixture.invocation("success");
    invocation.authority = vec!["workspace.read".into()];
    run_process_adapter(
        &fixture.adapter().with_environment(
            "CWR_CLAUDE_DANGER_FULL_ACCESS_WORKSPACE",
            fixture.directory.path().join("home"),
        ),
        invocation,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .expect_err("a workspace resource cannot grant native host access to itself");
}

#[test]
fn host_tool_round_trip_preserves_schema_and_does_not_replay_telemetry() {
    let fixture = Fixture::new();
    let mut calls = 0;
    let mut activities = 0;
    let invocation = fixture.invocation("call");
    let expected = invocation.host_operations[0].input_schema.clone();
    let outcome = run_process_adapter_with_activity_and_host_calls(
        &fixture.adapter(),
        invocation,
        &ProcessAdapterStopControl::new(),
        |_| {},
        |activity| {
            if activity.sequence != 0 {
                activities += 1;
            }
            Ok(())
        },
        |call| {
            calls += 1;
            assert_eq!(call.operation, "cwp.test--probe--value");
            assert_eq!(call.input, json!({"value":7}));
            thread::sleep(Duration::from_millis(50));
            Ok(json!({"doubled":14}))
        },
    )
    .unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
    assert_eq!((calls, activities), (1, 1));
    let tools: Value = serde_json::from_str(
        &fs::read_to_string(fixture.directory.path().join("tools.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(tools[0]["inputSchema"], expected);
    let reply: Value = serde_json::from_str(
        &fs::read_to_string(fixture.directory.path().join("reply.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(reply["result"]["structuredContent"], json!({"doubled":14}));
}

#[test]
fn host_errors_are_mcp_tool_errors() {
    let fixture = Fixture::new();
    run_process_adapter_with_activity_and_host_calls(
        &fixture.adapter(),
        fixture.invocation("call"),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |_| Ok(()),
        |_| Err("host refused the operation".into()),
    )
    .unwrap();
    let reply: Value = serde_json::from_str(
        &fs::read_to_string(fixture.directory.path().join("reply.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(reply["result"]["isError"], true);
}

#[test]
fn provider_progress_uses_phases_without_contents_or_duplicates() {
    use cwr::adapter::{AdapterActivityEvent, ExecutionPhase, ExecutionPhaseState};
    let fixture = Fixture::new();
    let mut summaries = Vec::new();
    let mut phases = Vec::new();
    run_process_adapter_with_activity_and_host_calls(
        &fixture.adapter(),
        fixture.invocation("progress"),
        &ProcessAdapterStopControl::new(),
        |progress| summaries.push(progress.summary),
        |activity| {
            if let AdapterActivityEvent::ExecutionActivity { activity } = activity.event {
                phases.push((activity.phase, activity.state));
            }
            Ok(())
        },
        |_| panic!("no calls expected"),
    )
    .unwrap();
    assert_eq!(summaries, ["Starting Claude."]);
    assert_eq!(
        phases,
        [
            (ExecutionPhase::Waiting, ExecutionPhaseState::Started),
            (ExecutionPhase::Thinking, ExecutionPhaseState::Started),
            (ExecutionPhase::Responding, ExecutionPhaseState::Started),
            (
                ExecutionPhase::CompactingContext,
                ExecutionPhaseState::Started
            ),
            (
                ExecutionPhase::CompactingContext,
                ExecutionPhaseState::Completed
            )
        ]
    );
}

#[test]
fn native_response_uses_only_terminal_text() {
    let fixture = Fixture::new();
    let mut invocation = fixture.invocation("native");
    invocation.action_name = "respond_to--message".into();
    invocation.output_schema =
        json!({"type":"object","required":["text"],"properties":{"text":{"type":"string"}}});
    let ProcessAdapterOutcome::Completed(result) = run_process_adapter(
        &fixture.adapter(),
        invocation,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap() else {
        panic!("expected completed")
    };
    assert_eq!(result.result, Some(json!({"text":"Final answer."})));
    assert!(
        !fs::read_to_string(fixture.directory.path().join("arguments"))
            .unwrap()
            .contains("--json-schema")
    );
}

#[test]
fn false_success_and_invalid_provider_output_are_rejected() {
    for scenario in [
        "bad-mcp",
        "missing-tool",
        "unexpected-tool",
        "bad-schema",
        "host-only-status",
        "missing-error",
        "both-payloads",
        "denied",
        "missing-result",
        "nonzero",
        "auth-failure",
        "wrong-session",
        "duplicate-result",
    ] {
        trace(format!("negative scenario={scenario} starting"));
        let fixture = Fixture::new();
        assert!(fixture.run(scenario).is_err(), "{scenario} must fail");
        assert_children_stopped(fixture.directory.path());
        trace(format!("negative scenario={scenario} finished"));
    }
}

#[test]
fn stop_and_timeout_clean_up_provider_and_mcp() {
    for scenario in ["stop", "ignore-term"] {
        trace(format!("cleanup scenario={scenario} starting"));
        let fixture = Fixture::new();
        let control = ProcessAdapterStopControl::new();
        let worker_control = control.clone();
        let adapter = fixture.adapter();
        let invocation = fixture.invocation(scenario);
        let worker = thread::spawn(move || {
            run_process_adapter(&adapter, invocation, &worker_control, |_| {})
        });
        wait_for(&fixture.directory.path().join("ready"));
        let started = Instant::now();
        control.stop();
        assert!(matches!(
            worker.join().unwrap().unwrap(),
            ProcessAdapterOutcome::Stopped
        ));
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_children_stopped(fixture.directory.path());
        trace(format!("cleanup scenario={scenario} finished"));
    }
    let fixture = Fixture::new();
    trace("cleanup timeout scenario starting");
    let mut invocation = fixture.invocation("stop");
    invocation.adapter_params["timeout_seconds"] = json!(1);
    assert!(run_process_adapter(
        &fixture.adapter(),
        invocation,
        &ProcessAdapterStopControl::new(),
        |_| {}
    )
    .is_err());
    assert_children_stopped(fixture.directory.path());
    trace("cleanup timeout scenario finished");
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "fixture did not become ready");
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_children_stopped(directory: &Path) {
    for name in ["provider.pid", "mcp.pid"] {
        let Ok(pid) = fs::read_to_string(directory.join(name)) else {
            continue;
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let output = std::process::Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", pid.trim()])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&output.stdout);
            if state.trim().is_empty() || state.trim().starts_with('Z') {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{name} {pid} still running: {state}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
