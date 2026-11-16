use cwr::adapter::{
    run_process_adapter, run_process_adapter_with_host_calls,
    run_process_adapter_with_model_output_activity_and_controlled_host_calls, AdapterActivityEvent,
    AdapterHostCallOutcome, AdapterHostOperation, AdapterInvocation, AdapterModelOutputMailbox,
    ProcessAdapterOutcome, ProcessAdapterStopControl, ResolvedProcessAdapter,
};
use cwr::open_responses::v2026_04_24::AssistantPhase;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("PROMPT.md"),
            "Use only the declared CWR functions.",
        )
        .unwrap();
        Self { directory }
    }

    fn adapter(&self) -> ResolvedProcessAdapter {
        ResolvedProcessAdapter::new(env!("CARGO_BIN_EXE_cwr-adapter-open-responses"))
            .with_current_directory(self.directory.path())
            .with_environment("CWR_COWORKER_DIRECTORY", self.directory.path())
            .with_environment("OPEN_RESPONSES_API_KEY", "test-secret")
    }

    fn invocation(&self, endpoint: &str) -> AdapterInvocation {
        let mut invocation = AdapterInvocation::new(
            "cwp://com.example/invocations/open-responses-test",
            "respond_to--message",
            json!({"text":"hello"}),
            json!({
                "type":"object",
                "additionalProperties":false,
                "required":["text"],
                "properties":{"text":{"type":"string"}}
            }),
        );
        invocation.model_run_ref = Some("cwp://com.example/model_runs/open-responses-test".into());
        invocation.thread = json!({"messages":[{"role":"user","text":"hello"}]});
        invocation.adapter_params = json!({
            "entrypoint":"./PROMPT.md",
            "model":"test/model",
            "max_turns":4,
            "timeout_seconds":10
        });
        invocation.integration_params = json!({"endpoint":endpoint});
        invocation
    }
}

#[test]
fn returns_assistant_text_and_sends_bearer_auth() {
    let server = MockServer::start(vec![MockReply::ok(response(vec![message(
        "msg_1",
        "Hello from Open Responses.",
    )]))]);
    let fixture = Fixture::new();
    let outcome = run_process_adapter(
        &fixture.adapter(),
        fixture.invocation(&server.endpoint),
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap();
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter did not complete")
    };
    assert_eq!(terminal.status, 200);
    assert_eq!(
        terminal.result,
        Some(json!({"text":"Hello from Open Responses."}))
    );
    assert_eq!(terminal.continuation, None);

    let requests = server.finish();
    assert!(requests[0]
        .headers
        .to_ascii_lowercase()
        .contains("authorization: bearer test-secret"));
    assert_eq!(requests[0].body["model"], "test/model");
    assert_eq!(requests[0].body["store"], false);
    assert_eq!(
        requests[0].body["include"],
        json!(["reasoning.encrypted_content"])
    );
    assert_eq!(requests[0].body["stream"], true);
}

#[test]
fn streams_commentary_and_assistant_text_before_the_terminal_response() {
    let server = GatedStreamServer::start();
    let fixture = Fixture::new();
    let adapter = fixture.adapter();
    let invocation = fixture.invocation(&server.endpoint);
    let mailbox = AdapterModelOutputMailbox::default();
    let runner_mailbox = mailbox.clone();
    let (outcome_sender, outcome_receiver) = mpsc::sync_channel(1);
    let (activity_sender, activity_receiver) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let outcome = run_process_adapter_with_model_output_activity_and_controlled_host_calls(
            &adapter,
            invocation,
            &ProcessAdapterStopControl::new(),
            &runner_mailbox,
            |_| {},
            |activity| {
                if activity.sequence != 0 {
                    activity_sender.send(activity).unwrap();
                }
                Ok(())
            },
            |_| -> cwr::CwrResult<AdapterHostCallOutcome> {
                panic!("streaming text test did not declare host calls")
            },
        );
        outcome_sender.send(outcome).unwrap();
    });

    server.wait_for_commentary_delta();
    let deadline = Instant::now() + Duration::from_secs(2);
    let commentary = loop {
        if let Some(output) = mailbox.snapshot().output {
            break output;
        }
        assert!(Instant::now() < deadline, "commentary was not projected");
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(commentary.text, "Checking");
    assert_eq!(commentary.phase, AssistantPhase::Commentary);

    server.release_commentary();
    let activity = activity_receiver
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    let AdapterActivityEvent::CompletedCommentary { commentary } = activity.event else {
        panic!("streamed reasoning did not complete as commentary")
    };
    assert_eq!(commentary.text, "Checking the request.");
    assert_eq!(commentary.response_or_turn_id, "resp_stream");

    server.wait_for_assistant_delta();
    let deadline = Instant::now() + Duration::from_secs(2);
    let assistant = loop {
        if let Some(output) = mailbox.snapshot().output {
            if output.phase == AssistantPhase::FinalAnswer {
                break output;
            }
        }
        assert!(
            Instant::now() < deadline,
            "assistant text was not projected"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(assistant.text, "Hello");
    assert_eq!(assistant.provider, "open-responses");

    server.release_assistant();
    let outcome = outcome_receiver
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter did not complete")
    };
    assert_eq!(terminal.result, Some(json!({"text":"Hello world."})));
    runner.join().unwrap();
    let request = server.finish();
    assert_eq!(request.body["stream"], true);
    assert!(request
        .headers
        .to_ascii_lowercase()
        .contains("accept: text/event-stream"));
}

#[test]
fn dispatches_declared_functions_and_replays_outputs_without_provider_storage() {
    let first = response(vec![
        json!({
            "type":"reasoning","id":"reason_1","status":"completed","summary":[]
        }),
        function_call(
            "fc_1",
            "call_1",
            "cwp__test--probe--value",
            json!({"value":7}),
        ),
    ]);
    let second = response(vec![message("msg_2", "The doubled value is 14.")]);
    let server = MockServer::start(vec![MockReply::ok(first), MockReply::ok(second)]);
    let fixture = Fixture::new();
    let mut invocation = fixture.invocation(&server.endpoint);
    invocation.host_operations = vec![AdapterHostOperation {
        name: "cwp.test--probe--value".into(),
        description: "Double a value.".into(),
        input_schema: json!({
            "type":"object","additionalProperties":false,"required":["value"],
            "properties":{"value":{"type":"integer"}}
        }),
        output_schema: json!({
            "type":"object","additionalProperties":false,"required":["doubled"],
            "properties":{"doubled":{"type":"integer"}}
        }),
    }];
    let outcome = run_process_adapter_with_host_calls(
        &fixture.adapter(),
        invocation,
        &ProcessAdapterStopControl::new(),
        |_| {},
        |call| {
            assert_eq!(call.operation, "cwp.test--probe--value");
            assert_eq!(call.input, json!({"value":7}));
            Ok(json!({"doubled":14}))
        },
    )
    .unwrap();
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter did not complete")
    };
    assert_eq!(terminal.status, 200);
    assert_eq!(
        terminal.result,
        Some(json!({"text":"The doubled value is 14."}))
    );

    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    let tools = requests[0].body["tools"].as_array().unwrap();
    assert_eq!(tools[0]["name"], "cwp__test--probe--value");
    let replay = requests[1].body["input"].as_array().unwrap();
    assert!(replay.iter().any(|item| item["type"] == "reasoning"));
    assert!(replay.iter().any(|item| item["type"] == "function_call"));
    let output = replay
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap();
    assert_eq!(output["call_id"], "call_1");
    assert_eq!(
        serde_json::from_str::<Value>(output["output"].as_str().unwrap()).unwrap(),
        json!({"result":{"doubled":14}})
    );
}

#[test]
fn named_actions_finish_through_the_reserved_return_function() {
    let returned = json!({
        "status":200,
        "summary":"Created the greeting.",
        "result":{"message":"hello"}
    });
    let server = MockServer::start(vec![MockReply::ok(response(vec![function_call(
        "fc_return",
        "call_return",
        "cwp__cwr--return--result",
        returned,
    )]))]);
    let fixture = Fixture::new();
    let mut invocation = fixture.invocation(&server.endpoint);
    invocation.action_name = "create--greeting".into();
    invocation.output_schema = json!({
        "type":"object","additionalProperties":false,"required":["message"],
        "properties":{"message":{"type":"string"}}
    });
    let outcome = run_process_adapter(
        &fixture.adapter(),
        invocation,
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap();
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter did not complete")
    };
    assert_eq!(terminal.status, 200);
    assert_eq!(terminal.summary, "Created the greeting.");
    assert_eq!(terminal.result, Some(json!({"message":"hello"})));

    let requests = server.finish();
    assert!(requests[0].body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "cwp__cwr--return--result"));
}

#[test]
fn preserves_structured_provider_errors() {
    let server = MockServer::start(vec![MockReply::json(
        401,
        json!({"error":{
            "type":"authentication_error",
            "code":"invalid_api_key",
            "message":"The API key is invalid."
        }}),
    )]);
    let fixture = Fixture::new();
    let outcome = run_process_adapter(
        &fixture.adapter(),
        fixture.invocation(&server.endpoint),
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap();
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter did not complete")
    };
    assert_eq!(terminal.status, 401);
    let error = terminal.error.unwrap();
    assert_eq!(error.code, "invalid_api_key");
    assert_eq!(error.message.as_deref(), Some("The API key is invalid."));
    assert_eq!(
        error.details.unwrap()["provider_error"]["type"],
        "authentication_error"
    );
    server.finish();
}

#[test]
fn turns_non_json_provider_failures_into_terminal_errors() {
    let server = MockServer::start(vec![MockReply::raw(503, "temporarily unavailable")]);
    let fixture = Fixture::new();
    let outcome = run_process_adapter(
        &fixture.adapter(),
        fixture.invocation(&server.endpoint),
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap();
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter did not complete")
    };
    assert_eq!(terminal.status, 503);
    let error = terminal.error.unwrap();
    assert_eq!(error.code, "http_503");
    assert_eq!(
        error.details.unwrap()["provider_body_preview"],
        "temporarily unavailable"
    );
    server.finish();
}

#[test]
fn bounds_large_provider_error_details() {
    let server = MockServer::start(vec![MockReply::json(
        400,
        json!({"error":{
            "code":"large_error",
            "message":"The provider rejected the request.",
            "metadata":"x".repeat(32 * 1024)
        }}),
    )]);
    let fixture = Fixture::new();
    let outcome = run_process_adapter(
        &fixture.adapter(),
        fixture.invocation(&server.endpoint),
        &ProcessAdapterStopControl::new(),
        |_| {},
    )
    .unwrap();
    let ProcessAdapterOutcome::Completed(terminal) = outcome else {
        panic!("adapter did not complete")
    };
    let details = terminal.error.unwrap().details.unwrap();
    assert_eq!(details["truncated"], true);
    assert!(details["original_bytes"].as_u64().unwrap() > 16 * 1024);
    server.finish();
}

include!("tests/support.rs");
