//! Optional native CLI smoke test against the checked-in local mock API.
use super::regression_tests::{start_action_operation, started};
use super::*;

#[test]
#[ignore = "requires CWR_CLAUDE_SMOKE_EXECUTABLE and a running local mock API"]
fn native_cli_independent_executions_and_structured_output() {
    let executable = std::env::var("CWR_CLAUDE_SMOKE_EXECUTABLE").unwrap();
    let base_url = std::env::var("CWR_CLAUDE_SMOKE_URL").unwrap();
    assert!(
        base_url
            .strip_prefix("http://127.0.0.1:")
            .and_then(|port| port.parse::<u16>().ok())
            .is_some_and(|port| port != 0),
        "smoke tests require a loopback URL with only a port"
    );
    for use_cli_login in [false, true] {
        run_profile(&executable, &base_url, use_cli_login);
    }
}

fn run_profile(executable: &str, base_url: &str, use_cli_login: bool) {
    let fixture = Fixture::new();
    let marker = fixture.directory.path().join("ambient-command-ran");
    let command = format!("touch '{}'", marker.display());
    let settings =
        json!({"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":command}]}]}});
    fs::create_dir(fixture.directory.path().join(".claude")).unwrap();
    for path in [".claude/settings.json", "home/settings.json"] {
        fs::write(fixture.directory.path().join(path), settings.to_string()).unwrap();
    }
    fs::write(
        fixture.directory.path().join("CLAUDE.md"),
        "CWR_UNTRUSTED_PROJECT_INSTRUCTIONS",
    )
    .unwrap();
    fs::write(
        fixture.directory.path().join(".mcp.json"),
        json!({"mcpServers":{"ambient":{
            "command":"/bin/sh","args":["-c",command]
        }}})
        .to_string(),
    )
    .unwrap();
    let adapter = fixture
        .adapter()
        .with_environment("ANTHROPIC_API_KEY", "local-probe-not-a-real-key")
        .with_environment("ANTHROPIC_BASE_URL", format!("{base_url}/start-action"));
    let mut invocation = fixture.invocation("claude-sonnet-4-6");
    invocation.integration_params =
        json!({"claude_executable":executable,"use_cli_login":use_cli_login});
    invocation.action_name = "respond_to--message".into();
    invocation.output_schema =
        json!({"type":"object","required":["text"],"properties":{"text":{"type":"string"}}});
    invocation.adapter_params["timeout_seconds"] = json!(30);
    invocation.host_operations = vec![start_action_operation()];
    let mut calls = 0;
    let mut summaries = Vec::new();
    let mut continuation = None;
    for _ in 0..2 {
        invocation.continuation = None;
        let ProcessAdapterOutcome::Completed(result) =
            run_process_adapter_with_activity_and_host_calls(
                &adapter,
                invocation.clone(),
                &ProcessAdapterStopControl::new(),
                |progress| summaries.push(progress.summary),
                |_| Ok(()),
                |call| {
                    calls += 1;
                    assert_eq!(call.operation, "cwr.start_action");
                    assert_eq!(
                        call.input,
                        json!({"activation":"@probe--double","input":{"value":7}})
                    );
                    Ok(started())
                },
            )
            .unwrap()
        else {
            panic!("expected completion")
        };
        assert_eq!(
            result.result,
            Some(json!({"text":"Probe finished: host result received."}))
        );
        continuation = result.continuation;
    }
    assert_eq!(calls, 2);
    invocation.continuation = continuation;
    run_process_adapter(
        &adapter,
        invocation.clone(),
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .expect_err("session continuation cannot account for interrupted live tool calls");
    for expected in ["Starting Claude.", "Starting @probe--double."] {
        assert!(summaries.iter().any(|summary| summary == expected));
    }
    invocation.continuation = None;
    invocation.action_name = "test--action".into();
    let structured = adapter
        .clone()
        .with_environment("ANTHROPIC_BASE_URL", format!("{base_url}/structured"));
    let ProcessAdapterOutcome::Completed(result) = run_process_adapter(
        &structured,
        invocation.clone(),
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap() else {
        panic!("expected structured completion")
    };
    assert_eq!(result.result, Some(json!({"text":"schema confirmed"})));
    let failure = adapter.with_environment("ANTHROPIC_BASE_URL", format!("{base_url}/model-error"));
    let ProcessAdapterOutcome::Completed(result) =
        run_process_adapter_with_activity_and_host_calls(
            &failure,
            invocation,
            &ProcessAdapterStopControl::new(),
            |_| {},
            |_| {
                thread::sleep(Duration::from_millis(300));
                Ok(())
            },
            |_| panic!("no host call expected on API rejection"),
        )
        .unwrap()
    else {
        panic!("expected provider failure")
    };
    assert_eq!(result.status, 502);
    assert!(result
        .error
        .unwrap()
        .message
        .unwrap()
        .contains("selected model is unavailable"));
    assert!(!marker.exists(), "ambient hooks or MCP commands ran");
}
