use super::*;
use cwr::adapter::AdapterActivityEvent;

fn private_pair(native: bool, managed_store: bool) {
    let fixture = Fixture::new();
    let mut adapter = fixture.adapter();
    if managed_store {
        adapter = adapter.with_environment("CWR_PROVIDER_STORE", "managed");
    }
    let mut invocation = fixture.invocation("commentary");
    if native {
        invocation.integration_params =
            json!({"claude_executable":std::env::var("CWR_CLAUDE_SMOKE_EXECUTABLE").unwrap()});
        invocation.adapter_params["model"] = json!("claude-sonnet-4-6");
        invocation.adapter_params["timeout_seconds"] = json!(45);
        let url = std::env::var("CWR_CLAUDE_SMOKE_URL").unwrap();
        assert!(url
            .strip_prefix("http://127.0.0.1:")
            .and_then(|p| p.parse::<u16>().ok())
            .is_some());
        adapter = adapter
            .with_environment("ANTHROPIC_API_KEY", "local-probe-not-a-real-key")
            .with_environment("ANTHROPIC_BASE_URL", url);
    }
    invocation.action_name = "respond_to--message".into();
    invocation.conversation_continuity = true;
    invocation.output_schema =
        json!({"type":"object","required":["text"],"properties":{"text":{"type":"string"}}});
    let mut state: Option<Value> = None;
    let mut calls = 0;
    let mut published = Vec::new();
    let mut tool_lifecycle = Vec::new();
    for turn in 0..2 {
        invocation.invocation_ref = format!("cwp://example.test/invocations/{turn}");
        invocation.model_run_ref = Some(format!("cwp://example.test/model_runs/{turn}"));
        invocation.thread = json!({"conversation_ref":"cwp://example.test/conversations/public",
            "context_snapshot":{"version":1,"message_count":turn+1},
            "messages":[{"text":if turn == 0 { "Read and remember the private probe result" } else { "FOLLOWUP correction-blue" }}],
            "working_conversation":{"ref":"cwp://example.test/conversations/working"}});
        if let Some(previous) = &state {
            invocation.thread["working_conversation"]["state"] = previous.clone();
        }
        let mut next = None;
        let ProcessAdapterOutcome::Completed(result) =
            run_process_adapter_with_activity_and_host_calls(
                &adapter,
                invocation.clone(),
                &ProcessAdapterStopControl::new(),
                |_| {},
                |activity| {
                    match activity.event {
                        AdapterActivityEvent::ProviderEvent { method, data, .. }
                            if method == "cwr.working_state" =>
                        {
                            next = Some(data)
                        }
                        AdapterActivityEvent::ProviderEvent { method, data, .. }
                            if method.starts_with("item/") =>
                        {
                            tool_lifecycle.push((method, data));
                        }
                        AdapterActivityEvent::CompletedCommentary { commentary } => {
                            published.push(commentary.text)
                        }
                        _ => {}
                    }
                    Ok(())
                },
                |_| {
                    calls += 1;
                    assert_eq!(turn, 0);
                    Ok(json!({"marker":"native-private-marker"}))
                },
            )
            .unwrap()
        else {
            panic!("completion")
        };
        assert_eq!(result.status, 200);
        let next = next.expect("settled native state");
        assert_eq!(next["version"], 1);
        if managed_store {
            assert_eq!(next["provider_store"], "managed");
        } else {
            assert!(next.get("provider_store").is_none());
        }
        if let Some(previous) = &state {
            assert_ne!(next["thread_id"], previous["thread_id"]);
        }
        state = Some(next);
        if native && turn == 1 {
            assert_eq!(
                result.result.unwrap()["text"],
                "native-private-marker correction-blue"
            );
        }
    }
    if native {
        assert_eq!(calls, 1);
        assert_eq!(
            tool_lifecycle.len(),
            2,
            "the real CLI must report one tool start and completion"
        );
        assert_eq!(tool_lifecycle[0].0, "item/started");
        assert_eq!(tool_lifecycle[1].0, "item/completed");
        assert_eq!(tool_lifecycle[1].1["item"]["status"], "completed");
    } else {
        assert_eq!(published.len(), 2);
        assert!(published
            .iter()
            .all(|text| text == "Created the environment; startup is unverified."));
        let args = fs::read_to_string(fixture.directory.path().join("arguments")).unwrap();
        assert!(
            args.contains("--fork-session\n") && args.contains(&format!("--resume\n{SESSION}\n"))
        );
        let observation =
            fs::read_to_string(fixture.directory.path().join("observation.json")).unwrap();
        assert!(observation.contains("FOLLOWUP correction-blue"));
        assert!(!observation.contains("Read and remember"));
    }
    if managed_store {
        invocation.thread["working_conversation"]["state"] = state.unwrap();
        assert!(run_process_adapter(
            &fixture.adapter(),
            invocation,
            &ProcessAdapterStopControl::new(),
            |_| {},
        )
        .is_err());
    }
}

#[test]
fn continuing_private_claude_forks_and_publishes_completed_text_before_tools() {
    private_pair(false, false);
}

#[test]
fn managed_claude_state_stays_managed_across_followups() {
    private_pair(false, true);
}

#[test]
#[ignore = "requires the native Claude CLI and native_mock.ts on a loopback port"]
fn native_claude_private_history_survives_process_restart() {
    private_pair(true, false);
}
