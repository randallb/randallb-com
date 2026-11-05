#[cfg(test)]
mod tests {
    use super::*;

    const MODEL_RUN_REF: &str = "cwp://org.coworkerprotocol/2026-09/model_runs/commentary-test";

    fn completed_assistant(phase: AssistantPhase, text: &str) -> CompletedAssistantMessage {
        CompletedAssistantMessage {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            item_id: "message-1".into(),
            phase,
            text: text.into(),
        }
    }

    fn command_event(method: &str, command: &str, exit_code: Option<i64>) -> Value {
        json!({
            "method": method,
            "params": {
                "item": {
                    "type": "commandExecution",
                    "command": command,
                    "status": if method == "item/started" { "inProgress" } else { "completed" },
                    "exitCode": exit_code
                }
            }
        })
    }

    #[test]
    fn provider_progress_classifies_commands_without_exposing_command_content() {
        let summary = command_progress_summary(&command_event(
            "item/started",
            "git status --short -- token=extremely-secret /private/repo",
            None,
        ))
        .expect("source-control progress");

        assert_eq!(summary, "Checking source-control changes.");
        assert!(!summary.contains("secret"));
        assert!(!summary.contains("/private"));
    }

    #[test]
    fn provider_progress_preserves_distinct_commands_with_the_same_category() {
        let first = command_event("item/started", "rg --files", None);
        let second = command_event("item/started", "grep -R needle .", None);

        assert_eq!(
            command_progress_summary(&first),
            Some("Searching the workspace.")
        );
        assert_eq!(
            command_progress_summary(&second),
            Some("Searching the workspace.")
        );
    }

    #[test]
    fn provider_progress_reports_failed_commands_but_not_successful_completions() {
        assert_eq!(
            command_progress_summary(&command_event("item/started", "cargo test", None)),
            Some("Running tests.")
        );
        assert_eq!(
            command_progress_summary(&command_event("item/completed", "cargo test", Some(0))),
            None
        );
        assert_eq!(
            command_progress_summary(&command_event("item/completed", "cargo test", Some(1))),
            Some("Test command failed; continuing.")
        );
    }

    #[test]
    fn completed_commentary_uses_the_exact_finished_item_and_provider_identity() {
        let completed = completed_assistant(AssistantPhase::Commentary, "  Exact text.  ");

        let commentary = completed_commentary(Some(completed), Some(MODEL_RUN_REF))
            .expect("completed commentary");

        assert_eq!(commentary.model_run_ref, MODEL_RUN_REF);
        assert_eq!(commentary.provider, "codex");
        assert_eq!(commentary.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(commentary.response_or_turn_id, "turn-1");
        assert_eq!(commentary.item_id, "message-1");
        assert_eq!(commentary.text, "  Exact text.  ");
    }

    #[test]
    fn only_nonblank_commentary_with_a_model_run_becomes_progress() {
        for completed in [
            completed_assistant(AssistantPhase::Commentary, ""),
            completed_assistant(AssistantPhase::Commentary, "  \n"),
            completed_assistant(AssistantPhase::FinalAnswer, "Finished"),
        ] {
            assert!(completed_commentary(Some(completed), Some(MODEL_RUN_REF)).is_none());
        }
        assert!(completed_commentary(
            Some(completed_assistant(AssistantPhase::Commentary, "Working")),
            None,
        )
        .is_none());
    }

    #[test]
    fn turn_started_scope_is_authoritative_and_the_rpc_response_must_agree() {
        let notification = json!({
            "method": "turn/started",
            "params": {
                "threadId": "thread-1",
                "turn": {"id": "turn-early"}
            }
        });
        let (thread_id, turn_id) = turn_started_scope(&notification).expect("early turn scope");
        let mut mapper = None;
        establish_open_responses_scope(
            &mut mapper,
            &thread_id,
            &turn_id,
            "turn/started notification",
        )
        .expect("establish early scope");
        establish_open_responses_scope(
            &mut mapper,
            "thread-1",
            "turn-early",
            "turn/start response",
        )
        .expect("matching RPC response");

        let error = establish_open_responses_scope(
            &mut mapper,
            "thread-1",
            "turn-late",
            "turn/start response",
        )
        .expect_err("mismatched RPC response must fail");
        assert!(error.to_string().contains("disagrees with active scope"));
        let mapper = mapper.expect("retained mapper");
        assert!(mapper.matches_scope("thread-1", "turn-early"));
    }

    #[test]
    fn only_the_active_turn_completion_ends_the_bounded_action() {
        let scope = CodexOpenResponsesMapper::new("parent-thread", "parent-turn");
        let nested = json!({
            "method": "turn/completed",
            "params": {
                "threadId": "child-thread",
                "turn": {"id": "child-turn", "status": "completed"}
            }
        });
        let parent = json!({
            "method": "turn/completed",
            "params": {
                "threadId": "parent-thread",
                "turn": {"id": "parent-turn", "status": "completed"}
            }
        });

        assert!(!turn_notification_matches_scope(&nested, Some(&scope)));
        assert!(turn_notification_matches_scope(&parent, Some(&scope)));
        assert!(!turn_notification_matches_scope(&parent, None));
    }

    #[test]
    fn thread_exposes_declared_host_tools_exactly() {
        let start_schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["activation"],
            "properties": {"activation": {"type": "string"}}
        });
        let await_schema = json!({
            "type": "object",
            "required": ["invocation_ref"],
            "properties": {"invocation_ref": {"type": "string"}}
        });
        let stop_schema = json!({
            "type": "object",
            "required": ["invocation_ref", "reason"],
            "properties": {
                "invocation_ref": {"type": "string"},
                "reason": {"type": "string"}
            }
        });
        let config = BoundedActionConfig {
            legacy_context_window_tokens: 128 * 1024,
            client_name: "test-client".into(),
            client_title: "Test Client".into(),
            model: None,
            reasoning_effort: None,
            service_tier_file: None,
            workspace: PathBuf::from("/tmp"),
            developer_instructions: "Test instructions".into(),
            observation: json!({}),
            return_schema: json!({"type": "object"}),
            native_text_result: false,
            provider_access: ProviderAccess::Normal,
            sandbox: "workspace-write",
            host_tools: vec![
                DynamicTool {
                    namespace: "cwp".into(),
                    name: "cwr--start--action".into(),
                    operation: "cwr.start_action".into(),
                    description: "Start one declared action.".into(),
                    input_schema: start_schema.clone(),
                },
                DynamicTool {
                    namespace: "cwp".into(),
                    name: "cwr--debug--invocation".into(),
                    operation: "cwr.inspect_invocation".into(),
                    description: "Await one child invocation.".into(),
                    input_schema: await_schema.clone(),
                },
                DynamicTool {
                    namespace: "cwp".into(),
                    name: "cwr--stop--invocation".into(),
                    operation: "cwr.stop_invocation".into(),
                    description: "Stop one invocation.".into(),
                    input_schema: stop_schema.clone(),
                },
            ],
            model_run_ref: None,
            continuation: None,
            provider_store: None,
            ephemeral: true,
        };
        let mut output = Vec::new();

        start_thread_with_resume(&mut output, &config, None, None).expect("start Codex thread");

        let request: Value = serde_json::from_slice(&output).expect("parse thread request");
        assert_eq!(request["params"]["dynamicTools"][0]["name"], "cwp");
        assert_eq!(
            request["params"]["config"]["features.code_mode.direct_only_tool_namespaces"],
            json!(["cwp"])
        );
        let tools = request["params"]["dynamicTools"][0]["tools"]
            .as_array()
            .expect("CWR namespace tools");
        assert_eq!(tools.len(), 4);
        assert_eq!(
            &tools[1..],
            &[
                json!({
                    "type": "function",
                    "name": "cwr--start--action",
                    "description": "Start one declared action.",
                    "inputSchema": start_schema
                }),
                json!({
                    "type": "function",
                    "name": "cwr--debug--invocation",
                    "description": "Await one child invocation.",
                    "inputSchema": await_schema
                }),
                json!({
                    "type": "function",
                    "name": "cwr--stop--invocation",
                    "description": "Stop one invocation.",
                    "inputSchema": stop_schema
                }),
            ]
        );
    }

    #[test]
    fn existing_threads_are_requested_without_hydrated_turns() {
        let config = BoundedActionConfig {
            legacy_context_window_tokens: 128 * 1024,
            client_name: "test-client".into(),
            client_title: "Test Client".into(),
            model: None,
            reasoning_effort: None,
            service_tier_file: None,
            workspace: PathBuf::from("/tmp"),
            developer_instructions: "Test instructions".into(),
            observation: json!({}),
            return_schema: json!({"type": "object"}),
            host_tools: Vec::new(),
            model_run_ref: None,
            continuation: None,
            provider_store: None,
            ephemeral: true,
            native_text_result: true,
            provider_access: ProviderAccess::Isolated,
            sandbox: "workspace-write",
        };

        for (resume_thread, fork_thread, method) in [
            (Some("resumed-thread"), None, "thread/resume"),
            (None, Some("forked-thread"), "thread/fork"),
        ] {
            let mut output = Vec::new();
            start_thread_with_resume(&mut output, &config, resume_thread, fork_thread).unwrap();
            let request: Value = serde_json::from_slice(&output).unwrap();

            assert_eq!(request["method"], method);
            assert_eq!(request["params"]["excludeTurns"], true);
            if fork_thread.is_some() {
                assert_eq!(request["params"]["threadSource"], "workloop");
            } else {
                assert!(request["params"].get("threadSource").is_none());
            }
        }
    }

    #[test]
    fn native_text_thread_registers_only_namespaces_with_tools() {
        for namespace in [None, Some("external")] {
            let config = BoundedActionConfig {
                legacy_context_window_tokens: 128 * 1024,
                client_name: "test-client".into(),
                client_title: "Test Client".into(),
                model: None,
                reasoning_effort: None,
            service_tier_file: None,
                workspace: PathBuf::from("/tmp"),
                developer_instructions: "Test instructions".into(),
                observation: json!({}),
                return_schema: json!({"type": "object"}),
                host_tools: namespace
                    .into_iter()
                    .map(|namespace| DynamicTool {
                        namespace: namespace.into(),
                        name: "lookup".into(),
                        operation: "external.lookup".into(),
                        description: "Look up an item.".into(),
                        input_schema: json!({"type": "object"}),
                    })
                    .collect(),
                model_run_ref: None,
                continuation: None,
                provider_store: None,
                ephemeral: true,
                native_text_result: true,
                provider_access: ProviderAccess::Isolated,
                sandbox: "workspace-write",
            };
            let mut output = Vec::new();
            start_thread_with_resume(&mut output, &config, None, None).unwrap();
            let request: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(request["method"], "thread/start");
            assert_eq!(request["params"]["threadSource"], "workloop");
            assert_eq!(request["params"]["sandbox"], "read-only");
            assert_eq!(request["params"]["config"]["web_search"], "disabled");
            assert_eq!(request["params"]["config"]["features.apps"], false);
            let namespaces = request["params"]["dynamicTools"].as_array().unwrap();
            assert_eq!(namespaces.len(), usize::from(namespace.is_some()));
            assert_eq!(
                request["params"]["config"]["features.code_mode.direct_only_tool_namespaces"],
                json!(namespace.into_iter().collect::<Vec<_>>())
            );
            if let Some(namespace) = namespace {
                assert_eq!(namespaces[0]["name"], namespace);
                let tools = namespaces[0]["tools"].as_array().unwrap();
                assert_eq!(tools.len(), 1);
                assert_eq!(tools[0]["name"], "lookup");
            }
        }
    }

    #[test]
    fn thread_can_run_without_the_codex_sandbox() {
        let config = BoundedActionConfig {
            legacy_context_window_tokens: 128 * 1024,
            client_name: "test-client".into(),
            client_title: "Test Client".into(),
            model: None,
            reasoning_effort: None,
            service_tier_file: None,
            workspace: PathBuf::from("/tmp"),
            developer_instructions: "Test instructions".into(),
            observation: json!({}),
            return_schema: json!({"type": "object"}),
            host_tools: Vec::new(),
            model_run_ref: None,
            continuation: None,
            provider_store: None,
            ephemeral: true,
            native_text_result: false,
            provider_access: ProviderAccess::Normal,
            sandbox: "danger-full-access",
        };
        let mut output = Vec::new();

        start_thread_with_resume(&mut output, &config, None, None).expect("start Codex thread");

        let request: Value = serde_json::from_slice(&output).expect("parse thread request");
        assert_eq!(request["params"]["approvalPolicy"], "never");
        assert_eq!(request["params"]["sandbox"], "danger-full-access");
    }

    #[test]
    fn restricted_thread_keeps_host_tools_without_workspace_or_network_access() {
        let config = BoundedActionConfig {
            legacy_context_window_tokens: 128 * 1024,
            client_name: "test-client".into(),
            client_title: "Test Client".into(),
            model: None,
            reasoning_effort: None,
            service_tier_file: None,
            workspace: PathBuf::from("/tmp"),
            developer_instructions: "Test instructions".into(),
            observation: json!({}),
            return_schema: json!({"type": "object"}),
            host_tools: vec![DynamicTool {
                namespace: "cwp".into(),
                name: "cwr--start--action".into(),
                operation: "cwr.start_action".into(),
                description: "Start one declared action.".into(),
                input_schema: json!({"type": "object"}),
            }],
            model_run_ref: None,
            continuation: None,
            provider_store: None,
            ephemeral: true,
            native_text_result: false,
            provider_access: ProviderAccess::CwrOnly,
            sandbox: "workspace-write",
        };
        let mut output = Vec::new();

        start_thread_with_resume(&mut output, &config, None, None)
            .expect("start restricted Codex thread");

        let request: Value = serde_json::from_slice(&output).expect("parse thread request");
        assert_eq!(request["params"]["sandbox"], Value::Null);
        assert_eq!(request["params"]["permissions"], "cwr-only");
        assert_eq!(
            request["params"]["config"]["default_permissions"],
            "cwr-only",
            "workspace requirements reload must select the same restricted profile"
        );
        assert_eq!(
            request["params"]["config"]["permissions.cwr-only.network.enabled"],
            false
        );
        assert_eq!(request["params"]["config"]["features.shell_tool"], false);
        assert_eq!(request["params"]["config"]["features.unified_exec"], false);
        assert_eq!(
            request["params"]["dynamicTools"][0]["tools"]
                .as_array()
                .unwrap()
                .len(),
            2,
            "restricted actions retain return_result and CWR host tools"
        );
    }
}

#[test]
fn execution_phases_publish_metadata_without_reasoning_or_provider_ids() {
    use crate::adapter::{ExecutionPhase, ExecutionPhaseState};
    let scope = CodexOpenResponsesMapper::new("thread", "turn");
    for (kind, phase) in [("reasoning", ExecutionPhase::Thinking), ("contextCompaction", ExecutionPhase::CompactingContext),
        ("commandExecution", ExecutionPhase::RunningCommand), ("mcpToolCall", ExecutionPhase::UsingTool), ("agentMessage", ExecutionPhase::Responding)] {
        for (method, state) in [("item/started", ExecutionPhaseState::Started), ("item/completed", ExecutionPhaseState::Completed)] {
            let event = json!({"method":method,"params":{"item":{"type":kind,"id":"private-id","text":"private text","reasoning":"private reasoning"}}});
            let activity = execution_activity(&event, Some(&scope)).unwrap();
            let AdapterActivityEvent::ExecutionActivity { activity: signal } = &activity else { panic!("typed phase") };
            assert_eq!(signal.phase, phase);
            assert_eq!(signal.state, state);
            assert!(!serde_json::to_string(&activity).unwrap().contains("private"));
        }
    }
    assert!(execution_activity(&json!({"method":"turn/started"}), Some(&scope)).is_none());
    assert!(execution_activity(&json!({"method":"item/reasoning/textDelta", "params":{"delta":"private"}}), Some(&scope)).is_none());
}

#[test]
fn nested_provider_items_do_not_change_the_active_worker_phase() {
    let scope = CodexOpenResponsesMapper::new("parent", "turn");
    let item = json!({"method":"item/started","params":{"threadId":"child","turnId":"child-turn","item":{"type":"reasoning"}}});
    assert!(execution_activity(&item, Some(&scope)).is_none());
    assert!(execution_activity(&item, None).is_none());
}

#[test]
fn live_compaction_activity_remains_scoped_to_the_active_turn() {
    use crate::adapter::{ExecutionPhase, ExecutionPhaseState};

    let scope = CodexOpenResponsesMapper::new("parent", "turn");
    for (method, state) in [
        ("item/started", ExecutionPhaseState::Started),
        ("item/completed", ExecutionPhaseState::Completed),
    ] {
        let active = json!({"method":method,"params":{"threadId":"parent","turnId":"turn",
            "item":{"type":"contextCompaction"}}});
        let AdapterActivityEvent::ExecutionActivity { activity } =
            execution_activity(&active, Some(&scope)).unwrap()
        else {
            panic!("typed live compaction phase")
        };
        assert_eq!(activity.phase, ExecutionPhase::CompactingContext);
        assert_eq!(activity.state, state);

        let unrelated = json!({"method":method,"params":{"threadId":"parent","turnId":"other-turn",
            "item":{"type":"contextCompaction"}}});
        assert!(execution_activity(&unrelated, Some(&scope)).is_none());
    }
}

#[cfg(test)]
#[derive(Default)]
struct TimingHandler {
    timings: Vec<crate::adapter::AdapterTiming>,
}

#[cfg(test)]
impl BoundedActionHandler for TimingHandler {
    fn on_timing(&mut self, timing: crate::adapter::AdapterTiming) {
        self.timings.push(timing);
    }
    fn on_progress(&mut self, _: u16, _: &str) -> CwrResult<()> {
        Ok(())
    }
    fn on_activity(&mut self, _: AdapterActivityEvent) -> CwrResult<()> {
        Ok(())
    }
    fn on_model_output(&mut self, _: AdapterModelOutput) -> CwrResult<()> {
        Ok(())
    }
    fn on_host_call(&mut self, _: &str, _: &str, _: Value) -> CwrResult<ToolCallResult> {
        panic!("timing test must not call tools")
    }
}

#[cfg(test)]
#[test]
fn live_compaction_failure_keeps_the_running_span_and_records_a_terminal_failure() {
    let scope = CodexOpenResponsesMapper::new("thread", "turn");
    let mut timings = TurnTimings::new(Instant::now());
    let mut handler = TimingHandler::default();
    let started = json!({"method":"item/started","params":{
        "threadId":"thread","turnId":"turn",
        "item":{"type":"contextCompaction","id":"compact-one"}}});
    timings.observe_live_compaction(&started, Instant::now(), &scope, &mut handler);
    timings.fail_live_compaction(Instant::now(), &mut handler);

    assert_eq!(handler.timings.len(), 2);
    let start = &handler.timings[0];
    assert_eq!(
        start.checkpoint,
        crate::adapter::TimingCheckpoint::LiveCompactionStarted
    );
    assert_eq!(
        start.compaction.as_ref().unwrap().outcome,
        crate::adapter::AdapterCompactionOutcome::Running
    );
    assert!(start.durations_ms.is_empty());
    let failed = &handler.timings[1];
    assert_eq!(
        failed.checkpoint,
        crate::adapter::TimingCheckpoint::LiveCompactionFailed
    );
    assert_eq!(
        failed.compaction.as_ref().unwrap().outcome,
        crate::adapter::AdapterCompactionOutcome::Failed
    );
    assert!(failed
        .durations_ms
        .contains_key(&crate::adapter::TimingDuration::ContextCompaction));
}
