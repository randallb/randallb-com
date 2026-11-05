use super::*;
use cwr::adapter::{
    run_process_adapter_with_model_output_activity_and_controlled_host_calls, AdapterActivityEvent,
    AdapterModelOutputMailbox, ExecutionPhase, ExecutionPhaseState,
};

#[test]
fn native_activity_and_text_preserve_identity_without_private_payloads() {
    let fixture = Fixture::new();
    let mut invocation = fixture.invocation("presentation");
    invocation.authority = vec!["process.execute".into()];
    invocation.action_name = "respond_to--message".into();
    invocation.model_run_ref = Some("cwp://example.test/model-runs/presentation".into());
    invocation.output_schema =
        json!({"type":"object","required":["text"],"properties":{"text":{"type":"string"}}});
    let capture = fixture.directory.path().join("capture-adapter");
    fs::write(
        &capture,
        format!(
            "#!/bin/sh\n'{}' | /usr/bin/tee adapter-events\n",
            env!("CARGO_BIN_EXE_cwr-adapter-claude").replace('\'', "'\\''")
        ),
    )
    .unwrap();
    fs::set_permissions(&capture, fs::Permissions::from_mode(0o700)).unwrap();
    let mailbox = AdapterModelOutputMailbox::default();
    let mut activities = Vec::new();

    let outcome = run_process_adapter_with_model_output_activity_and_controlled_host_calls(
        &fixture.adapter_executable(&capture),
        invocation,
        &ProcessAdapterStopControl::new(),
        &mailbox,
        |_| {},
        |activity| {
            if let AdapterActivityEvent::CompletedCommentary { commentary } = &activity.event {
                assert_eq!(commentary.item_id, "commentary");
                assert_eq!(commentary.text, "Checking the workspace.");
            }
            activities.push(activity.event);
            Ok(())
        },
        |_| panic!("tool telemetry must not execute host work"),
    )
    .unwrap();
    let ProcessAdapterOutcome::Completed(result) = outcome else {
        panic!("completion")
    };
    assert_eq!(result.result, Some(json!({"text":"Final answer."})));
    assert!(
        mailbox.snapshot().revision > 0,
        "model output must be accepted by the mailbox"
    );
    let frames: Vec<Value> = fs::read_to_string(fixture.directory.path().join("adapter-events"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let commentary_frames: Vec<_> = frames
        .iter()
        .filter(|frame| frame["type"] == "model_output" && frame["output"]["phase"] == "commentary")
        .collect();
    assert_eq!(commentary_frames.len(), 5);
    for frame in commentary_frames {
        assert_eq!(frame["output"]["response_or_turn_id"], "commentary");
    }
    let encoded = serde_json::to_string(&activities).unwrap();
    for secret in [
        "PRIVATE_THINKING",
        "SECRET_ARGUMENT",
        "SECRET_RESULT",
        "foreign-tool",
    ] {
        assert!(!encoded.contains(secret));
    }
    let tools: Vec<_> = activities
        .iter()
        .filter_map(|event| match event {
            AdapterActivityEvent::ProviderEvent { method, data, .. }
                if method.starts_with("item/") =>
            {
                Some((method.as_str(), data))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        tools.len(),
        2,
        "duplicate and unknown events must not invent lifecycle"
    );
    assert_eq!(tools[0].0, "item/started");
    assert_eq!(tools[0].1["item"]["tool"], "Bash");
    assert_eq!(tools[0].1["item"]["progress_summary"], "Running a command.");
    let commentary_index = activities
        .iter()
        .position(|event| matches!(event, AdapterActivityEvent::CompletedCommentary { .. }))
        .unwrap();
    let tool_index = activities.iter().position(|event| matches!(event, AdapterActivityEvent::ProviderEvent { method, .. } if method == "item/started")).unwrap();
    assert!(
        commentary_index < tool_index,
        "commentary must precede its tool"
    );
    assert_eq!(tools[1].0, "item/completed");
    assert_eq!(tools[1].1["item"]["status"], "failed");
    assert!(activities.iter().any(|event| matches!(event,
        AdapterActivityEvent::ExecutionActivity { activity }
        if activity.phase == ExecutionPhase::Thinking && activity.state == ExecutionPhaseState::Completed)));
    assert_children_stopped(fixture.directory.path());
}

#[test]
fn parallel_tool_results_do_not_complete_another_tool_or_newer_text_phase() {
    let fixture = Fixture::new();
    let mut phases = Vec::new();
    let mut tools = Vec::new();
    run_process_adapter_with_activity_and_host_calls(
        &fixture.adapter(),
        fixture.invocation("parallel-tools"),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |activity| {
            match activity.event {
                AdapterActivityEvent::ExecutionActivity { activity } => {
                    phases.push((activity.phase, activity.state))
                }
                AdapterActivityEvent::ProviderEvent { method, data, .. }
                    if method.starts_with("item/") =>
                {
                    tools.push((method, data))
                }
                _ => {}
            }
            Ok(())
        },
        |_| panic!("telemetry must not execute work"),
    )
    .unwrap();
    assert_eq!(
        phases,
        vec![
            (ExecutionPhase::RunningCommand, ExecutionPhaseState::Started),
            (ExecutionPhase::Responding, ExecutionPhaseState::Started),
            (ExecutionPhase::Responding, ExecutionPhaseState::Completed),
        ]
    );
    assert_eq!(tools.len(), 4);
    assert_eq!(tools[2].1["item"]["id"], "a");
    assert_eq!(tools[3].1["item"]["id"], "b");
}

#[test]
fn mixed_tools_restore_the_remaining_phase_and_use_projectable_items() {
    let fixture = Fixture::new();
    let mut phases = Vec::new();
    let mut tools = Vec::new();
    run_process_adapter_with_activity_and_host_calls(
        &fixture.adapter(),
        fixture.invocation("mixed-tools"),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |activity| {
            match activity.event {
                AdapterActivityEvent::ExecutionActivity { activity } => {
                    phases.push((activity.phase, activity.state))
                }
                AdapterActivityEvent::ProviderEvent { method, data, .. }
                    if method.starts_with("item/") =>
                {
                    tools.push(data)
                }
                _ => {}
            }
            Ok(())
        },
        |_| panic!("telemetry must not execute work"),
    )
    .unwrap();
    assert_eq!(
        phases,
        vec![
            (ExecutionPhase::RunningCommand, ExecutionPhaseState::Started),
            (ExecutionPhase::UsingTool, ExecutionPhaseState::Started),
            (ExecutionPhase::RunningCommand, ExecutionPhaseState::Started),
            (
                ExecutionPhase::RunningCommand,
                ExecutionPhaseState::Completed
            ),
        ]
    );
    assert_eq!(tools.len(), 4);
    for event in [&tools[1], &tools[2]] {
        assert_eq!(event["item"]["type"], "dynamicToolCall");
        assert_eq!(event["item"]["tool"], "Read");
    }
}

#[test]
fn host_dispatch_evidence_suppresses_only_represented_tool_activity() {
    for scenario in ["host-before", "host-after", "host-rejected"] {
        for native in [false, true] {
            let fixture = Fixture::new();
            let mut invocation = fixture.invocation(scenario);
            invocation.host_operations = vec![AdapterHostOperation {
                name: if native {
                    "cwp.alice--resize--photo"
                } else {
                    "cwr.send_message"
                }
                .into(),
                description: "Exercise represented activity".into(),
                input_schema: if native {
                    json!({"type":"object","required":["value"],"properties":{"value":{"type":"integer"}},"x-cwr-target":{"action_name":"resize--photo"}})
                } else {
                    json!({"type":"object","required":["value"],"properties":{"value":{"type":"integer"}}})
                },
                output_schema: json!({"type":"object"}),
            }];
            if native {
                invocation.host_operations.push(AdapterHostOperation {
                    name: "cwr.invoke_action".into(), description: "Dispatch native action".into(),
                    input_schema: json!({"type":"object","required":["target","input"],"properties":{"target":{"type":"object"},"input":{"type":"object"}}}),
                    output_schema: json!({"type":"object"}),
                });
            }
            let mut tools = Vec::new();
            let mut calls = 0;
            run_process_adapter_with_activity_and_host_calls(
                &fixture.adapter(),
                invocation,
                &ProcessAdapterStopControl::new(),
                |_| {},
                |activity| {
                    if let AdapterActivityEvent::ProviderEvent { method, data, .. } = activity.event
                    {
                        if method.starts_with("item/") {
                            tools.push((method, data));
                        }
                    }
                    Ok(())
                },
                |call| {
                    calls += 1;
                    assert_eq!(
                        call.operation,
                        if native {
                            "cwr.invoke_action"
                        } else {
                            "cwr.send_message"
                        }
                    );
                    Ok(json!({}))
                },
            )
            .unwrap();
            if scenario == "host-rejected" {
                assert_eq!(calls, 0);
                assert_eq!(tools.len(), 2);
                assert_eq!(tools[1].1["item"]["status"], "failed");
            } else {
                assert_eq!(calls, 1);
                assert!(tools.is_empty());
            }
        }
    }
}
