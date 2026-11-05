use super::*;
use cwr::action_contract::{start_action_input_schema, start_action_output_schema};

pub(super) fn start_action_operation() -> AdapterHostOperation {
    let variants: Vec<_> = [
        ("@probe--double", json!({"type":"object","required":["value"],"additionalProperties":false,"properties":{"value":{"type":"integer","minimum":1}}})),
        ("@probe--greet", json!({"type":"object","required":["name"],"additionalProperties":false,"properties":{"name":{"type":"string"}}})),
    ].into_iter().map(|(activation, input)| {
        let mut variant = start_action_input_schema();
        variant["properties"]["activation"] = json!({"const":activation});
        variant["properties"]["input"] = input;
        variant
    }).collect();
    AdapterHostOperation {
        name: "cwr.start_action".into(),
        description: "Start @probe--double or @probe--greet.".into(),
        input_schema: json!({"oneOf":variants}),
        output_schema: start_action_output_schema(),
    }
}

pub(super) fn started() -> Value {
    json!({"invocation_ref":"cwp://com.example/invocations/started","state":"admitted","queued":true})
}

#[test]
fn start_action_exposes_object_but_validates_exact_activation_input_pair() {
    let fixture = Fixture::new();
    let mut invocation = fixture.invocation("start-action");
    invocation.host_operations = vec![start_action_operation()];
    let mut calls = 0;
    let mut summaries = Vec::new();
    run_process_adapter_with_activity_and_host_calls(
        &fixture.adapter(),
        invocation,
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
    .unwrap();
    assert_eq!(
        calls, 1,
        "invalid calls must be rejected before reaching the host"
    );
    assert!(summaries.contains(&"Starting @probe--double.".into()));
    let tools: Value = serde_json::from_str(
        &fs::read_to_string(fixture.directory.path().join("tools.json")).unwrap(),
    )
    .unwrap();
    let schema = &tools[0]["inputSchema"];
    assert_eq!(schema["type"], "object");
    for keyword in ["oneOf", "anyOf", "allOf"] {
        assert!(schema.get(keyword).is_none());
    }
    for (activation, input) in [
        ("@probe--double", json!({"value":7})),
        ("@probe--greet", json!({"name":"Ada"})),
    ] {
        assert!(jsonschema::is_valid(
            schema,
            &json!({"activation":activation,"input":input})
        ));
    }
}

#[test]
fn provider_failures_preserve_bounded_details_after_delayed_activity_ack() {
    for (scenario, details) in [
        ("api-error-result", vec!["selected model is unavailable"]),
        (
            "execution-error",
            vec![
                "error_during_execution",
                "HTTP 429",
                "Provider capacity exhausted",
                "Please retry later",
            ],
        ),
        ("long-error", vec!["bad model", "😶", "…"]),
    ] {
        let fixture = Fixture::new();
        let ProcessAdapterOutcome::Completed(result) =
            run_process_adapter_with_activity_and_host_calls(
                &fixture.adapter(),
                fixture.invocation(scenario),
                &ProcessAdapterStopControl::new(),
                |_| {},
                |_| {
                    // The provider has already emitted its failure and exited before
                    // CWR acknowledges the initial session activity.
                    thread::sleep(Duration::from_millis(300));
                    Ok(())
                },
                |_| panic!("no host calls expected"),
            )
            .unwrap()
        else {
            panic!("expected failed terminal")
        };
        assert_eq!(result.status, 502);
        assert!(result.result.is_none());
        assert!(result.continuation.is_none());
        let error = result.error.unwrap();
        assert_eq!(error.code, "claude_provider_error");
        let message = error.message.unwrap();
        for detail in details {
            assert!(message.contains(detail), "{message}");
        }
        assert!(message.chars().count() <= 1025);
        assert!(!message.chars().any(char::is_control));
        assert!(!message.contains("MUST_NOT_APPEAR_IN_DIAGNOSTICS"));
        assert!(!message.contains("Final answer."));
        assert_children_stopped(fixture.directory.path());
    }
}
