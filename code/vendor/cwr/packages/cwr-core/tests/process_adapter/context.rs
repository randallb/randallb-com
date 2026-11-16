use super::*;

#[test]
fn exact_sixteen_mib_input_and_result_cross_the_process_boundary() {
    let script = format!(
        r##"#!/usr/bin/env python3
import json
import sys

request = json.loads(sys.stdin.readline())
assert len(json.dumps(request["invocation"]["input"], separators=(",", ":")).encode()) == {MAX_ACTION_VALUE_BYTES}
result = "x" * ({MAX_ACTION_VALUE_BYTES} - 2)
print(json.dumps({{
    "type": "result",
    "invocation_ref": "{INVOCATION_REF}",
    "sequence": 1,
    "status": 200,
    "summary": "Finished",
    "result": result,
}}, separators=(",", ":")), flush=True)
"##
    );
    let (directory, executable) = fixture(&script);
    let marker = directory.path().join("unused");
    let mut request = AdapterInvocation::new(
        INVOCATION_REF,
        "test--action",
        Value::String("x".repeat(MAX_ACTION_VALUE_BYTES - 2)),
        json!({"type": "string"}),
    );
    request.thread = json!({});

    let outcome = run_process_adapter(
        &adapter(&executable, &marker),
        request,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap();
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("exact-limit request must complete")
    };
    let result = terminal.result.unwrap();
    assert_eq!(
        serde_json::to_vec(&result).unwrap().len(),
        MAX_ACTION_VALUE_BYTES
    );
}

#[test]
fn execution_start_must_be_persisted_before_adapter_launch() {
    let (directory, executable) = fixture("#!/bin/sh\n: > \"$1\"\n");
    let marker = directory.path().join("started");
    let mut activities = Vec::new();
    let error = cwr::adapter::run_process_adapter_with_activity_and_host_calls(
        &adapter(&executable, &marker),
        invocation(false),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |activity| {
            activities.push(activity);
            Err(std::io::Error::other("persistence failed").into())
        },
        |_| unreachable!("no host call before launch"),
    )
    .expect_err("execution marker persistence failure prevents execution");
    assert!(error.to_string().contains("persistence failed"));
    assert!(!marker.exists());
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].sequence, 0);
    assert!(matches!(&activities[0].event,
        cwr::adapter::AdapterActivityEvent::ProviderEvent { provider, method, .. }
        if provider == "cwr" && method == "cwr.execution_started"));
}

#[test]
fn oversized_control_is_rejected_before_starting_the_adapter() {
    let (directory, executable) = fixture("#!/bin/sh\n: > \"$1\"\n");
    let marker = directory.path().join("started");
    let mut request = invocation(false);
    request.input = json!({"body": "x".repeat(MAX_ACTION_VALUE_BYTES)});
    let error = run_process_adapter(
        &adapter(&executable, &marker),
        request,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .expect_err("oversized invocation control");
    assert!(error.to_string().contains("16 MiB serialized JSON limit"));
    assert!(!marker.exists());
}

#[test]
fn stop_interrupts_context_delivery_to_an_adapter_that_does_not_read() {
    assert_context_delivery_is_interrupted("#!/bin/sh\n: > \"$1\"\nexec /bin/sleep 30\n");
}

#[test]
fn stop_interrupts_context_delivery_after_an_adapter_stops_reading() {
    // Announce readiness after consuming the first context frame, so this case
    // measures interrupted delivery separately from large-string preparation.
    assert_context_delivery_is_interrupted(
        "#!/bin/sh\nIFS= read -r invoke\nIFS= read -r context\n: > \"$1\"\nexec /bin/sleep 30\n",
    );
}

fn assert_context_delivery_is_interrupted(script: &str) {
    let (directory, executable) = fixture(script);
    let marker = directory.path().join("started");
    let adapter = adapter(&executable, &marker);
    let stop = ProcessAdapterStopControl::new();
    let runner_stop = stop.clone();
    let mut request = invocation(false);
    request.thread = json!({"messages": [{"text": "x".repeat(16 * 1024 * 1024)}]});
    let runner =
        thread::spawn(move || run_process_adapter(&adapter, request, &runner_stop, |_| {}));
    wait_for_marker(&marker);
    let started = Instant::now();
    stop.stop();
    assert_eq!(
        runner.join().unwrap().unwrap(),
        ProcessAdapterOutcome::Stopped
    );
    let stop_elapsed = started.elapsed();
    assert!(
        stop_elapsed < Duration::from_secs(1),
        "stop waited {stop_elapsed:?} after adapter readiness while delivering 16 MiB of context",
    );
}

#[test]
fn oversized_settled_host_reply_reports_delivery_failure_without_repeating_work() {
    let (directory, executable) = fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"call-1","operation":"cwr.test","input":{{}}}}'
IFS= read -r reply
printf '%s' "$reply" > "$1"
printf '%s\n' '{{"type":"result","invocation_ref":"{INVOCATION_REF}","sequence":1,"status":200,"summary":"Observed delivery failure","result":{{}}}}'
"##
    ));
    let marker = directory.path().join("reply");
    let mut completed_operations = 0;
    let outcome = run_process_adapter_with_controlled_host_calls(
        &adapter(&executable, &marker),
        invocation(true),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |_| {
            completed_operations += 1;
            Ok(AdapterHostCallOutcome::Returned(
                json!({"body": "x".repeat(MAX_ACTION_VALUE_BYTES)}),
            ))
        },
    )
    .unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
    assert_eq!(completed_operations, 1);
    let reply: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(marker).unwrap()).unwrap();
    assert_eq!(reply["error"]["code"], "result_delivery_failed");
    assert!(reply.get("result").is_none());
}

#[test]
fn exact_nested_action_result_crosses_the_host_reply_envelope() {
    let (directory, executable) = fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"call-1","operation":"cwr.invoke_action","input":{{"input":{{}}}}}}'
IFS= read -r reply
printf '%s' "$reply" > "$1"
printf '%s\n' '{{"type":"result","invocation_ref":"{INVOCATION_REF}","sequence":1,"status":200,"summary":"Observed exact result","result":{{}}}}'
"##
    ));
    let marker = directory.path().join("reply");
    let mut request = invocation(false);
    request.host_operations.push(AdapterHostOperation {
        name: "cwr.invoke_action".into(),
        description: "Invoke one action".into(),
        input_schema: json!({
            "x-cwr-decoder-version": "cwp/1",
            "type": "object",
            "required": ["input"],
            "properties": {"input": {}},
            "additionalProperties": false
        }),
        output_schema: json!({}),
    });
    let response = json!({
        "invocation_ref": INVOCATION_REF,
        "sequence": 1,
        "status": 200,
        "summary": "Finished",
        "result": Value::String("x".repeat(MAX_ACTION_VALUE_BYTES - 2))
    });
    let outcome = run_process_adapter_with_controlled_host_calls(
        &adapter(&executable, &marker),
        request,
        &ProcessAdapterStopControl::new(),
        |_| {},
        |_| Ok(AdapterHostCallOutcome::Returned(response.clone())),
    )
    .unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
    let reply: Value = serde_json::from_str(&fs::read_to_string(marker).unwrap()).unwrap();
    assert!(reply.get("error").is_none());
    assert_eq!(
        serde_json::to_vec(&reply["result"]["result"])
            .unwrap()
            .len(),
        MAX_ACTION_VALUE_BYTES
    );
}

#[test]
fn failed_nested_action_response_uses_the_control_frame_limit() {
    let (directory, executable) = fixture(&format!(
        r##"#!/bin/sh
IFS= read -r invoke
printf '%s\n' '{{"type":"call","invocation_ref":"{INVOCATION_REF}","call_id":"call-1","operation":"cwr.invoke_action","input":{{"input":{{}}}}}}'
IFS= read -r reply
printf '%s' "$reply" > "$1"
printf '%s\n' '{{"type":"result","invocation_ref":"{INVOCATION_REF}","sequence":1,"status":200,"summary":"Observed failed response","result":{{}}}}'
"##
    ));
    let marker = directory.path().join("reply");
    let mut request = invocation(false);
    request.host_operations.push(AdapterHostOperation {
        name: "cwr.invoke_action".into(),
        description: "Invoke one action".into(),
        input_schema: json!({
            "x-cwr-decoder-version": "cwp/1",
            "type": "object",
            "required": ["input"],
            "properties": {"input": {}},
            "additionalProperties": false
        }),
        output_schema: json!({}),
    });
    let response = json!({
        "invocation_ref": INVOCATION_REF,
        "sequence": 1,
        "status": 500,
        "summary": "Failed",
        "error": {
            "code": "provider_failed",
            "details": "x".repeat(MAX_ACTION_VALUE_BYTES)
        }
    });
    assert!(serde_json::to_vec(&response).unwrap().len() > MAX_ACTION_VALUE_BYTES);
    let outcome = run_process_adapter_with_controlled_host_calls(
        &adapter(&executable, &marker),
        request,
        &ProcessAdapterStopControl::new(),
        |_| {},
        |_| Ok(AdapterHostCallOutcome::Returned(response.clone())),
    )
    .unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
    let reply: Value = serde_json::from_str(&fs::read_to_string(marker).unwrap()).unwrap();
    assert!(reply.get("error").is_none());
    assert_eq!(
        reply["result"]["error"]["details"]
            .as_str()
            .expect("failed action details")
            .len(),
        MAX_ACTION_VALUE_BYTES
    );
}
