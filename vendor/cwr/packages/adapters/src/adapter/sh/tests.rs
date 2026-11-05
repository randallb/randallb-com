mod sh_tests {
    use std::io::Cursor;
    use std::io::BufReader as CoworkerReader;

    use super::*;

    include!("runner_tests.rs");
    include!("gateway_tests.rs");
    include!("timeout_tests.rs");
    include!("host_call_frame_tests.rs");

    include!("groupme_contract_tests.rs");
    include!("discovery_diagnostics_tests.rs");

    #[test]
    fn configured_command_runs_for_an_argumentless_action() {
        assert_configured_command(json!({"command": "printf configured"}));
    }

    #[test]
    fn action_command_takes_precedence_over_default_and_other_actions() {
        assert_configured_command(json!({
            "command": "exit 41",
            "commands": {
                "run--configured_action": "printf configured",
                "create--tmux_pane": "exit 42"
            }
        }));
    }

    #[test]
    fn unmatched_action_command_keeps_the_default() {
        assert_configured_command(json!({
            "command": "printf configured",
            "commands": {"create--tmux_pane": "exit 42"}
        }));
    }

    #[test]
    fn check_commands_fail_without_losing_diagnostics() {
        let event = configured_command_result(json!({
            "command": "printf output; printf diagnostic >&2; exit 7",
            "fail_on_nonzero": true
        }));
        assert!(matches!(event, AdapterEvent::Result {
            status: 500, result: None, error: Some(error), ..
        } if error.code == "command_failed" && error.details.as_ref().is_some_and(|details|
            details["exit_code"] == 7 && details["stdout"] == "output"
                && details["stderr"] == "diagnostic")));
    }

    #[test]
    fn check_commands_pass_on_zero_exit() {
        assert_configured_command(json!({
            "command": "printf configured", "fail_on_nonzero": true
        }));
    }

    #[test]
    fn streamed_output_remains_available_in_the_terminal_result() {
        let event = configured_command_result(json!({
            "command": "printf streamed; printf diagnostic >&2",
            "stream_output": true
        }));
        assert!(matches!(event, AdapterEvent::Result {
            status: 200, result: Some(result), error: None, ..
        } if result["stdout"] == "streamed" && result["stderr"] == "diagnostic"));
    }

    #[test]
    fn configured_command_receives_action_input_as_json_on_stdin() {
        let event = configured_command_result_with_input(
            json!({
                "command": "read -r payload; printf '%s' \"$payload\""
            }),
            json!({"subject": "docs(repo): explain commits"}),
        );
        assert!(matches!(event, AdapterEvent::Result {
            status: 200, result: Some(result), error: None, ..
        } if result["stdout"] == "{\"subject\":\"docs(repo): explain commits\"}"));
    }

    #[test]
    fn command_exit_before_stdin_write_is_not_an_adapter_failure() {
        struct ClosedStdin;

        impl Write for ClosedStdin {
            fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        write_shell_action_input(ClosedStdin, &json!({})).unwrap();
    }

    #[test]
    fn configured_command_can_emit_opt_in_progress_on_stderr() {
        let events = configured_command_events_with_input(
            json!({
                "command": "printf 'CWR_PROGRESS\\t{\"summary\":\"Waiting for rust.\"}\\n' >&2; printf done; printf diagnostic >&2",
                "progress_jsonl_stderr_actions": ["run--configured_action"]
            }),
            json!({}),
        );
        assert!(matches!(&events[1], AdapterEvent::Progress {
            sequence: 2, status: 102, published: false, progress: None, summary, ..
        } if summary == "Waiting for rust."));
        assert!(matches!(&events[2], AdapterEvent::Result {
            sequence: 3, result: Some(result), ..
        } if result["stdout"] == "done" && result["stderr"] == "diagnostic"));
    }

    #[test]
    fn ordinary_stderr_cannot_emit_progress() {
        let events = configured_command_events_with_input(
            json!({
                "command": "printf 'CWR_PROGRESS\\t{\"summary\":\"Not progress.\"}\\n' >&2"
            }),
            json!({}),
        );
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[1], AdapterEvent::Result {
            sequence: 2, result: Some(result), ..
        } if result["stderr"] == "CWR_PROGRESS\t{\"summary\":\"Not progress.\"}\n"));
    }

    #[test]
    fn invalid_authored_progress_is_diagnostic_without_failing_the_action() {
        for payload in [
            json!({"summary":"update", "published":"yes"}),
            json!({"summary":"update", "progress":{"key":"tests", "completed":1}}),
            json!({"summary":"update", "progress":null}),
            json!({"summary":"two\nlines", "published":true}),
        ] {
            let line = format!("CWR_PROGRESS\t{payload}\n");
            let events = configured_command_events_with_input(json!({
                "command": format!("printf '%s' '{}' >&2; printf done", line),
                "progress_jsonl_stderr_actions":["run--configured_action"]
            }), json!({}));
            assert_eq!(events.len(), 2);
            assert!(matches!(&events[1], AdapterEvent::Result {
                status: 200, result: Some(result), ..
            } if result["stderr"] == line && result["stdout"] == "done"));
        }
    }

    #[test]
    fn ordinary_commands_keep_nonzero_exit_as_a_successful_result() {
        for params in [
            json!({"command": "exit 7"}),
            json!({"command": "exit 7", "fail_on_nonzero": false}),
        ] {
            assert!(
                matches!(configured_command_result(params), AdapterEvent::Result {
                status: 200, result: Some(result), error: None, ..
            } if result["exit_code"] == 7)
            );
        }
    }

    #[test]
    fn allowlisted_message_runs_its_fixed_command() {
        let event = message_command_result(
            json!({"message_commands": {"start": "printf started"}}),
            "@workloop start",
        );
        assert!(matches!(event, AdapterEvent::Result {
            status: 200, result: Some(result), error: None, ..
        } if result["text"].as_str().is_some_and(|text|
            text.contains("`printf started` exited with code 0.")
                && text.contains("started"))));
    }

    #[test]
    fn configured_message_command_can_receive_source_and_previous_messages() {
        let previous_ref = "cwp://com.example.runtime/conversations/one/messages/previous";
        let source_ref = "cwp://com.example.runtime/conversations/one/messages/source";
        let mut invocation = AdapterInvocation::new(
            "cwp://com.example.runtime/invocations/message-context",
            RESPOND_TO_MESSAGE_ACTION,
            json!({"source_message_ref": source_ref}),
            json!({"type": "object"}),
        );
        invocation.actor_integration_params = json!({
            "command": "read -r payload; printf '%s' \"$payload\"",
            "message_context_json_stdin": true
        });
        invocation.thread = json!({"messages": [
            {"message_ref": previous_ref, "text": "See `notes/example.md`."},
            {"message_ref": source_ref, "text": "@typora"}
        ]});
        let event = run_invocation(invocation);
        assert!(matches!(event, AdapterEvent::Result {
            status: 200, result: Some(result), error: None, ..
        } if result["text"].as_str().is_some_and(|text| {
            let payload = text
                .split("stdout:\n```")
                .nth(1)
                .and_then(|value| value.strip_suffix("\n```"))
                .and_then(|value| serde_json::from_str::<Value>(value).ok());
            payload.is_some_and(|payload| {
                payload["previous_message"]["message_ref"] == previous_ref
                    && payload["previous_message"]["text"] == "See `notes/example.md`."
                    && payload["source_message"]["message_ref"] == source_ref
                    && payload["source_message"]["text"] == "@typora"
            })
        })));
    }

    #[test]
    fn unknown_allowlisted_message_returns_available_names() {
        let event = message_command_result(
            json!({"message_commands": {"start": "exit 99"}}),
            "@workloop stop",
        );
        assert!(matches!(event, AdapterEvent::Result {
            status: 200, result: Some(result), error: None, ..
        } if result["text"] == "Available commands: start."));
    }

    #[test]
    fn configured_command_receives_the_invocation_ref() {
        let event = configured_command_result(json!({
            "command": "printf '%s' \"$CWR_INVOCATION_REF\""
        }));
        assert!(matches!(event, AdapterEvent::Result {
            status: 200, result: Some(result), ..
        } if result["stdout"] ==
            "cwp://com.example.runtime/invocations/configured-shell-command"));
    }

    #[test]
    fn configured_command_receives_the_retained_source_actor_ref() {
        let event = configured_command_result(json!({
            "command": "printf '%s' \"$CWR_SOURCE_ACTOR_REF\""
        }));
        assert!(matches!(event, AdapterEvent::Result {
            status: 200, result: Some(result), ..
        } if result["stdout"] == "~@/actors/requester"));
    }

    #[test]
    fn coworker_bridge_waits_for_one_visible_action() {
        let mut bridge = ShellCoworkerBridge::bind(&invoke_operation()).unwrap();
        let socket = bridge.socket().to_path_buf();
        let client = thread::spawn(move || {
            let mut stream = CoworkerStream::connect(socket).unwrap();
            let mut reader = CoworkerReader::new(stream.try_clone().unwrap());
            write_adapter_message(
                &mut stream,
                &json!({"type":"describe_action", "activation":"@helper--run--task"}),
            )
            .unwrap();
            let described: Value = read_adapter_message(&mut reader).unwrap().unwrap();
            assert_eq!(described["type"], "action");
            assert_eq!(
                described["input_schema"]["properties"]["count"]["type"],
                "integer"
            );
            write_adapter_message(
                &mut stream,
                &json!({
                    "type":"invoke_action",
                    "activation":"@helper--run--task",
                    "input":{"count":3}
                }),
            )
            .unwrap();
            read_adapter_message::<Value>(&mut reader).unwrap().unwrap()
        });
        let call_result = AdapterHostMessage::CallResult {
            invocation_ref: "~@/invocations/parent".into(),
            call_id: SHELL_ACTION_CALL_ID.into(),
            result: Some(json!({
                "invocation_ref":"~@/invocations/child",
                "sequence":2,
                "status":200,
                "summary":"Finished.",
                "result":{"ok":true}
            })),
            error: None,
        };
        let mut input = Cursor::new(format!(
            "{}\n{}\n",
            discovery_response(),
            serde_json::to_string(&call_result).unwrap()
        ));
        let mut output = Vec::new();
        let mut events = ShellAdapterEvents {
            gateway_operation: None,
            coworker_actions_available: true,
            input: ShellHostInput::Direct(&mut input),
            output: &mut output,
            invocation_ref: "~@/invocations/parent",
            source_actor_ref: None,
            next_sequence: 2,
            progress_fd: None,
        };
        poll_bridge_until_handled(&mut bridge, &mut events);
        assert_eq!(
            client.join().unwrap()["result"]["invocation_ref"],
            "~@/invocations/child"
        );
        let call: AdapterEvent =
            serde_json::from_str(String::from_utf8(output).unwrap().lines().last().unwrap()).unwrap();
        assert!(matches!(call, AdapterEvent::Call {
            invocation_ref, call_id, operation, input
        } if invocation_ref == "~@/invocations/parent"
            && call_id == SHELL_ACTION_CALL_ID
            && operation == crate::action_contract::INVOKE_ACTION_HOST_OPERATION
            && input["target"] == exact_target()
            && input["input"] == json!({"count":3})
            && input["timeout_ms"].as_u64().is_some_and(|timeout| timeout <= 12_345)));
    }

    #[test]
    fn coworker_bridge_accepts_an_async_action_receipt() {
        let mut bridge = ShellCoworkerBridge::bind(&invoke_operation()).unwrap();
        let socket = bridge.socket().to_path_buf();
        let client = thread::spawn(move || {
            let mut stream = CoworkerStream::connect(socket).unwrap();
            let mut reader = CoworkerReader::new(stream.try_clone().unwrap());
            write_adapter_message(
                &mut stream,
                &json!({"type":"describe_action", "activation":"@helper--run--task"}),
            )
            .unwrap();
            let _: Value = read_adapter_message(&mut reader).unwrap().unwrap();
            write_adapter_message(
                &mut stream,
                &json!({
                    "type":"invoke_action",
                    "activation":"@helper--run--task",
                    "input":{}
                }),
            )
            .unwrap();
            read_adapter_message::<Value>(&mut reader).unwrap().unwrap()
        });
        let call_result = AdapterHostMessage::CallResult {
            invocation_ref: "~@/invocations/parent".into(),
            call_id: SHELL_ACTION_CALL_ID.into(),
            result: Some(json!({
                "invocation_ref":"~@/invocations/child",
                "state":"admitted",
                "queued":true
            })),
            error: None,
        };
        let mut input = Cursor::new(format!(
            "{}\n{}\n",
            discovery_response(),
            serde_json::to_string(&call_result).unwrap()
        ));
        let mut output = Vec::new();
        let mut events = ShellAdapterEvents {
            gateway_operation: None,
            coworker_actions_available: true,
            input: ShellHostInput::Direct(&mut input),
            output: &mut output,
            invocation_ref: "~@/invocations/parent",
            source_actor_ref: None,
            next_sequence: 2,
            progress_fd: None,
        };
        poll_bridge_until_handled(&mut bridge, &mut events);
        assert_eq!(client.join().unwrap()["type"], "started");
        let call: AdapterEvent =
            serde_json::from_str(String::from_utf8(output).unwrap().lines().last().unwrap()).unwrap();
        assert!(matches!(call, AdapterEvent::Call { operation, input, .. }
            if operation == crate::action_contract::INVOKE_ACTION_HOST_OPERATION
                && input.get("attachment").is_none()));
    }

    #[test]
    fn coworker_bridge_rejects_another_connection_after_starting() {
        let mut bridge = ShellCoworkerBridge::bind(&invoke_operation()).unwrap();
        bridge.handled = true;
        let socket = bridge.socket().to_path_buf();
        let client = thread::spawn(move || {
            let stream = CoworkerStream::connect(socket).unwrap();
            read_adapter_message::<Value>(&mut CoworkerReader::new(stream))
                .unwrap()
                .unwrap()
        });
        let mut input = Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        let mut events = ShellAdapterEvents {
            gateway_operation: None,
            coworker_actions_available: true,
            input: ShellHostInput::Direct(&mut input),
            output: &mut output,
            invocation_ref: "~@/invocations/parent",
            source_actor_ref: None,
            next_sequence: 2,
            progress_fd: None,
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !client.is_finished() {
            bridge.poll(&mut events, deadline).unwrap();
            assert!(
                Instant::now() < deadline,
                "bridge did not reject second connection"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(client.join().unwrap()["type"], "error");
        assert!(output.is_empty());
    }

    #[test]
    fn coworker_bridge_rejects_an_action_that_is_not_visible() {
        let mut bridge = ShellCoworkerBridge::bind(&invoke_operation()).unwrap();
        let socket = bridge.socket().to_path_buf();
        let client = thread::spawn(move || {
            let mut stream = CoworkerStream::connect(socket).unwrap();
            write_adapter_message(
                &mut stream,
                &json!({"type":"describe_action", "activation":"@hidden--run--task"}),
            )
            .unwrap();
            read_adapter_message::<Value>(&mut CoworkerReader::new(stream))
                .unwrap()
                .unwrap()
        });
        let mut input = Cursor::new(format!("{}\n", discovery_response()));
        let mut output = Vec::new();
        let mut events = ShellAdapterEvents {
            gateway_operation: None,
            coworker_actions_available: true,
            input: ShellHostInput::Direct(&mut input),
            output: &mut output,
            invocation_ref: "~@/invocations/parent",
            source_actor_ref: None,
            next_sequence: 2,
            progress_fd: None,
        };
        poll_bridge_until_handled(&mut bridge, &mut events);
        let response = client.join().unwrap();
        assert_eq!(response["type"], "error");
        assert_eq!(response["diagnostic"], json!({
            "stage": "discovery", "reason": "action_unavailable",
            "catalog_actions": 1, "matching_actions": 0, "default_matching_actions": 0
        }));
        let calls: Vec<AdapterEvent> = String::from_utf8(output).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(calls.len(), 1);
        assert!(matches!(&calls[0], AdapterEvent::Call { operation, .. } if operation == "cwr.list_actions"));
    }

    #[test]
    fn coworker_socket_disappears_with_its_shell_command() {
        let bridge = ShellCoworkerBridge::bind(&invoke_operation()).unwrap();
        let socket = bridge.socket().to_path_buf();
        drop(bridge);
        assert!(CoworkerStream::connect(socket).is_err());
    }

    #[test]
    fn signal_termination_fails_a_check() {
        assert!(matches!(configured_command_result(json!({
            "command": "kill -TERM $$", "fail_on_nonzero": true
        })), AdapterEvent::Result { status: 500, error: Some(error), .. }
            if error.details.as_ref().unwrap()["exit_code"] == -1));
    }

    fn assert_configured_command(params: Value) {
        assert!(matches!(
            configured_command_result(params),
            AdapterEvent::Result {
                status: 200,
                result: Some(result),
                ..
            } if result["command"] == "printf configured"
                && result["exit_code"] == 0
                && result["stdout"] == "configured"
        ));
    }

    fn configured_command_result(params: Value) -> AdapterEvent {
        configured_command_result_with_input(params, json!({}))
    }

    fn configured_command_result_with_input(params: Value, action_input: Value) -> AdapterEvent {
        configured_command_events_with_input(params, action_input)
            .pop()
            .expect("terminal event")
    }

    fn configured_command_events_with_input(
        params: Value,
        action_input: Value,
    ) -> Vec<AdapterEvent> {
        let mut invocation = AdapterInvocation::new(
            "cwp://com.example.runtime/invocations/configured-shell-command",
            "run--configured_action",
            action_input,
            json!({"type": "object"}),
        );
        invocation.source_actor_ref = Some("~@/actors/requester".into());
        invocation.actor_integration_params = params;
        let invoke = AdapterHostMessage::Invoke {
            protocol: PROCESS_ADAPTER_PROTOCOL_REF.into(),
            invocation: Box::new(invocation),
        };
        let input = format!("{}\n", serde_json::to_string(&invoke).unwrap());
        let mut output = Vec::new();

        run_sh_adapter(Cursor::new(input), &mut output).unwrap();

        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<AdapterEvent>(line).unwrap())
            .collect()
    }

    fn message_command_result(params: Value, text: &str) -> AdapterEvent {
        let message_ref = "cwp://com.example.runtime/conversations/one/messages/one";
        let mut invocation = AdapterInvocation::new(
            "cwp://com.example.runtime/invocations/message-command",
            RESPOND_TO_MESSAGE_ACTION,
            json!({"source_message_ref": message_ref}),
            json!({"type": "object"}),
        );
        invocation.actor_integration_params = params;
        invocation.thread = json!({
            "messages": [{"message_ref": message_ref, "text": text}]
        });
        run_invocation(invocation)
    }

    fn run_invocation(invocation: AdapterInvocation) -> AdapterEvent {
        let input = format!(
            "{}\n",
            serde_json::to_string(&AdapterHostMessage::Invoke {
                protocol: PROCESS_ADAPTER_PROTOCOL_REF.into(),
                invocation: Box::new(invocation),
            })
            .unwrap()
        );
        let mut output = Vec::new();
        run_sh_adapter(Cursor::new(input), &mut output).unwrap();
        serde_json::from_str(String::from_utf8(output).unwrap().lines().last().unwrap()).unwrap()
    }

    fn exact_target() -> Value {
        json!({"runtime_ref":"cwp://example/loop", "coworker_ref":"cwp://example/helper",
            "role_ref":"cwp://example/role", "action_name":"run--task",
            "binding_ref":"cwp://example/helper/bindings/main", "contract_id":"original"})
    }

    fn discovery_response() -> Value {
        json!({"type":"call_result","invocation_ref":"~@/invocations/parent",
            "call_id":"shell-coworker-list-1", "result":{"actions":[{
                "activation":"@helper--run--task", "target":exact_target(),
                "is_default_binding":true,
                "input_schema":{"type":"object","properties":{"count":{"type":"integer"}}}
            }]}})
    }

    fn invoke_operation() -> AdapterHostOperation {
        AdapterHostOperation {
            name: crate::action_contract::INVOKE_ACTION_HOST_OPERATION.into(),
            description: "Invoke one visible action.".into(),
            input_schema: {
                let mut schema = crate::action_contract::invoke_action_input_schema();
                schema["x-cwr-action-input-schemas"] = json!({
                    "@helper--run--task": {
                        "type":"object",
                        "properties":{"count":{"type":"integer"}}
                    }
                });
                schema
            },
            output_schema: crate::action_contract::invoke_action_output_schema(),
        }
    }

    fn poll_bridge_until_handled(
        bridge: &mut ShellCoworkerBridge,
        events: &mut ShellAdapterEvents<'_, impl Write>,
    ) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !bridge.handled {
            bridge.poll(events, deadline).unwrap();
            assert!(
                Instant::now() < deadline,
                "bridge did not receive coworker request"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
