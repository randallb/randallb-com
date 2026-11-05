#[test]
fn two_codex_adapters_exchange_one_message_through_the_host() {
    let (caller_directory, caller_codex) = fixture(FAKE_CALLER_CODEX);
    let (helper_directory, helper_codex) = fixture(FAKE_HELPER_CODEX);
    let mut caller_invocation = invocation(&caller_codex);
    caller_invocation.invocation_ref =
        "cwp://org.coworkerprotocol/2026-09/invocations/caller-turn".into();
    caller_invocation
        .host_operations
        .push(send_message_operation());
    let caller_stop = ProcessAdapterStopControl::new();
    let mut delivered_to_helper = Vec::new();
    let outcome = run_process_adapter_with_host_calls(
        &codex_adapter(caller_directory.path()),
        caller_invocation,
        &caller_stop,
        |_| {},
        |call| {
            delivered_to_helper.push(call.input.clone());
            let mut helper_invocation = invocation(&helper_codex);
            helper_invocation.invocation_ref =
                "cwp://org.coworkerprotocol/2026-09/invocations/helper-turn".into();
            helper_invocation.input = json!({"message": call.input["text"]});
            helper_invocation.thread = json!({
                "messages": [{
                    "sender": "@caller",
                    "text": call.input["text"]
                }]
            });
            let helper_outcome = run_process_adapter(
                &codex_adapter(helper_directory.path()),
                helper_invocation,
                &ProcessAdapterStopControl::new(),
                |_| {},
            )?;
            let ProcessAdapterOutcome::Completed(helper_terminal) = helper_outcome else {
                return Err(std::io::Error::other("helper adapter stopped").into());
            };
            let reply = helper_terminal
                .result
                .as_ref()
                .and_then(|result| result["message"].as_str())
                .ok_or_else(|| std::io::Error::other("helper returned no message"))?;
            Ok(json!({
                "messages": [{"sender": "@helper", "text": reply}]
            }))
        },
    )
    .expect("exchange one message through two Codex adapters");
    assert_eq!(delivered_to_helper.len(), 1);
    assert_eq!(delivered_to_helper[0]["to"], "@helper");
    assert_eq!(delivered_to_helper[0]["text"], "question from caller");
    let ProcessAdapterOutcome::Completed(caller_terminal) = outcome else {
        panic!("caller adapter stopped unexpectedly");
    };
    assert_eq!(
        caller_terminal.result,
        Some(json!({"message": "caller received helper reply"}))
    );
}
#[test]
fn codex_adapter_pauses_for_an_action_and_resumes_with_its_result() {
    let (directory, fake_codex) = fixture(FAKE_ACTION_CALLER_CODEX);
    let mut requested = invocation(&fake_codex);
    requested.host_operations.push(invoke_action_operation());
    let mut host_calls = Vec::new();
    let outcome = run_process_adapter_with_host_calls(
        &codex_adapter(directory.path()),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
        |call| {
            host_calls.push(call.clone());
            Ok(json!({
                "notation": "1d20",
                "rolls": [14],
                "modifier": 0,
                "total": 14,
                "text": "Rolled 1d20: 14. Total: 14."
            }))
        },
    )
    .expect("invoke an RNG action and resume the caller");
    assert_eq!(host_calls.len(), 1);
    assert_eq!(host_calls[0].call_id, "roll-dice-1");
    assert_eq!(host_calls[0].operation, "cwr.invoke_action");
    assert_eq!(host_calls[0].input["activation"], "@rng--roll--dice");
    assert_eq!(
        host_calls[0].input["input"],
        json!({"count": 1, "sides": 20, "modifier": 0})
    );
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("action caller adapter stopped unexpectedly");
    };
    assert_eq!(
        terminal.result,
        Some(json!({"message": "caller continued after rolling 14"}))
    );
    assert_eq!(terminal.continuation.unwrap()["thread_id"], "action-caller-thread");
}
#[test]
fn codex_adapter_dispatches_arbitrary_declared_host_operations_exactly() {
    let (directory, fake_codex) = fixture(FAKE_GENERIC_HOST_TOOLS_CODEX);
    let mut requested = invocation(&fake_codex);
    requested.host_operations = vec![
        dynamic_operation(
            "cwr.start_action",
            "Start one declared action.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["action_name"],
                "properties": {"action_name": {"type": "string"}}
            }),
            json!({
                "type": "object",
                "required": ["invocation_ref"],
                "properties": {"invocation_ref": {"type": "string"}}
            }),
        ),
        dynamic_operation(
            "cwr.inspect_invocation",
            "Await one child invocation.",
            json!({
                "type": "object",
                "required": ["invocation_ref"],
                "properties": {"invocation_ref": {"type": "string"}}
            }),
            json!({
                "type": "object",
                "required": ["status"],
                "properties": {"status": {"type": "integer"}}
            }),
        ),
        dynamic_operation(
            "cwr.stop_invocation",
            "Stop one invocation.",
            json!({
                "type": "object",
                "required": ["invocation_ref"],
                "properties": {"invocation_ref": {"type": "string"}}
            }),
            json!({
                "type": "object",
                "required": ["stopped"],
                "properties": {"stopped": {"type": "boolean"}}
            }),
        ),
    ];
    let mut calls = Vec::new();

    let outcome = run_process_adapter_with_controlled_host_calls(
        &codex_adapter(directory.path()),
        requested,
        &ProcessAdapterStopControl::new(),
        |_| {},
        |call| {
            calls.push(call.clone());
            let result = match call.operation.as_str() {
                "cwr.start_action" => json!({"invocation_ref": "child-1"}),
                "cwr.inspect_invocation" => json!({"status": 200}),
                "cwr.stop_invocation" => json!({"stopped": true}),
                operation => panic!("unexpected host operation {operation}"),
            };
            Ok(AdapterHostCallOutcome::Returned(result))
        },
    )
    .expect("dispatch generic Codex host tools");

    assert_eq!(
        calls
            .iter()
            .map(|call| {
                (
                    call.call_id.clone(),
                    call.operation.clone(),
                    call.input.clone(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (
                "start-1".to_owned(),
                "cwr.start_action".to_owned(),
                json!({"action_name": "draft--brief"})
            ),
            (
                "await-1".to_owned(),
                "cwr.inspect_invocation".to_owned(),
                json!({"invocation_ref": "child-1"})
            ),
            (
                "stop-1".to_owned(),
                "cwr.stop_invocation".to_owned(),
                json!({"invocation_ref": "child-2"})
            ),
        ]
    );
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
}


#[test]
fn retained_legacy_helper_decodes_its_original_target_through_the_process_transport() {
    let provider = FAKE_ACTION_CALLER_CODEX.replace(
        "\"namespace\":\"cwp\",\"tool\":\"cwr--invoke--action\"",
        "\"namespace\":\"cwr\",\"tool\":\"invoke_action\"",
    );
    let (directory, executable) = fixture(&provider);
    let mut requested = invocation(&executable);
    let target = json!({"runtime_ref":"cwp://com.example/original-loop", "coworker_ref":"cwp://com.example/original-rng",
        "role_ref":"cwp://com.example/dice-role", "action_name":"roll--dice",
        "binding_ref":"cwp://com.example/original-rng/bindings/original", "contract_id":"original-contract"});
    let mut legacy = invoke_action_operation();
    legacy.input_schema["x-cwr-model-name"] = json!("cwr.invoke_action");
    legacy.input_schema["x-cwr-decoder-version"] = json!("cwr/1");
    legacy.input_schema["x-cwr-action-targets"] = json!({"@rng--roll--dice":target});
    requested.host_operations = vec![legacy.clone()];
    let mut calls = Vec::new();
    let outcome = run_process_adapter_with_host_calls(&codex_adapter(directory.path()), requested,
        &ProcessAdapterStopControl::new(), |_| {}, |call| {
            calls.push(call.clone());
            Ok(json!({"notation":"1d20", "rolls":[14], "modifier":0, "total":14, "text":"Rolled 14"}))
        }).unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].operation, "cwr.invoke_action");
    assert_eq!(calls[0].input["target"], target);
    assert!(calls[0].input.get("activation").is_none());
    assert_eq!(legacy.input_schema["properties"]["activation"]["const"], "@rng--roll--dice");
}
