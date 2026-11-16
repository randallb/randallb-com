#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoopbackParams {
    #[serde(default)]
    delay_ms: u64,
    #[serde(default = "default_true")]
    report_progress: bool,
    #[serde(default)]
    host_call: Option<LoopbackHostCall>,
    #[serde(default)]
    host_calls: Vec<LoopbackHostCall>,
    #[serde(default)]
    mode: Option<LoopbackMode>,
    #[serde(default)]
    response_text: Option<String>,
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum LoopbackMode {
    Feedback,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoopbackHostCall {
    operation: String,
    input: Value,
}
pub fn run_loopback_adapter(mut input: impl BufRead, mut output: impl Write) -> CwrResult<()> {
    let message: AdapterHostMessage = read_adapter_message(&mut input)?
        .ok_or_else(|| protocol_error("loopback adapter received no invocation"))?;
    let (protocol, invocation) = match message {
        AdapterHostMessage::Invoke {
            protocol,
            invocation,
        } => (protocol, invocation),
        AdapterHostMessage::CallResult { .. } => {
            return protocol_failure("loopback adapter received a call result before invocation")
        }
        AdapterHostMessage::ActivityResult { .. } => {
            return protocol_failure(
                "loopback adapter received an activity result before invocation",
            )
        }
    };
    if protocol != PROCESS_ADAPTER_PROTOCOL_REF {
        return protocol_failure(format!(
            "loopback adapter supports {PROCESS_ADAPTER_PROTOCOL_REF}, not {protocol}"
        ));
    }
    validate_adapter_invocation(&invocation)?;
    let params: LoopbackParams = serde_json::from_value(crate::adapter::activated::params(&invocation.adapter_params))
        .map_err(|error| protocol_error(format!("loopback adapter params are invalid: {error}")))?;
    let mut sequence = 1;
    if params.report_progress {
        write_adapter_message(
            &mut output,
            &AdapterEvent::Progress {
                progress: None,
                published: false,
                invocation_ref: invocation.invocation_ref.clone(),
                sequence,
                status: 102,
                summary: "Looping back the action input.".into(),
            },
        )?;
        sequence += 1;
    }
    if params.delay_ms > 0 {
        thread::sleep(Duration::from_millis(params.delay_ms));
    }
    let (result, error, status, summary) = if let Some(text) = params.response_text {
        if invocation.action_name != RESPOND_TO_MESSAGE_ACTION {
            return protocol_failure("loopback response_text only supports message responses");
        }
        (
            Some(json!({"text": text})),
            None,
            200,
            "Returned the configured message response.".into(),
        )
    } else if params.mode == Some(LoopbackMode::Feedback) {
        let result = feedback_result(&invocation)?;
        (Some(result), None, 200, "Recorded feedback.".into())
    } else if !params.host_calls.is_empty() {
        let mut calls = Vec::new();
        for (index, host_call) in params.host_calls.into_iter().enumerate() {
            let call_id = format!("loopback-call-{}", index + 1);
            let (result, error) = loopback_host_call(
                &mut input, &mut output, &invocation.invocation_ref, &call_id, host_call,
            )?;
            match (result, error) {
                (Some(result), None) => calls.push(json!({"call_id": call_id, "result": result})),
                (None, Some(error)) => return write_adapter_message(&mut output, &AdapterEvent::Result {
                    invocation_ref: invocation.invocation_ref,
                    sequence, status: 500, summary: "The host operation failed.".into(),
                    result: None, error: Some(error), continuation: invocation.continuation,
                }),
                _ => return protocol_failure("loopback host call result is invalid"),
            }
        }
        (Some(json!({"calls": calls, "adapter_pid": std::process::id()})), None, 200,
         "Returned sequential host operation results.".into())
    } else if let Some(host_call) = params.host_call {
        let (result, error) = loopback_host_call(
            &mut input, &mut output, &invocation.invocation_ref, "loopback-call-1", host_call,
        )?;
        match (result, error) {
            (Some(result), None) => (Some(result), None, 200, "Returned the host operation result.".into()),
            (None, Some(error)) => (None, Some(error), 500, "The host operation failed.".into()),
            _ => return protocol_failure("loopback host call result is invalid"),
        }
    } else {
        (
            Some(invocation.input),
            None,
            200,
            "Returned the action input.".into(),
        )
    };
    write_adapter_message(
        &mut output,
        &AdapterEvent::Result {
            invocation_ref: invocation.invocation_ref,
            sequence,
            status,
            summary,
            result,
            error,
            continuation: invocation.continuation,
        },
    )
}

fn loopback_host_call(
    input: &mut impl BufRead,
    output: &mut impl Write,
    invocation_ref: &str,
    call_id: &str,
    host_call: LoopbackHostCall,
) -> CwrResult<(Option<Value>, Option<AdapterError>)> {
    write_adapter_message(output, &AdapterEvent::Call {
        invocation_ref: invocation_ref.into(), call_id: call_id.into(),
        operation: host_call.operation, input: host_call.input,
    })?;
    match read_adapter_message(input)? {
        Some(AdapterHostMessage::CallResult {
            invocation_ref: response_ref, call_id: response_id, result, error,
        }) if response_ref == invocation_ref && response_id == call_id => Ok((result, error)),
        _ => protocol_failure("loopback adapter received the wrong host call result"),
    }
}

fn feedback_result(invocation: &AdapterInvocation) -> CwrResult<Value> {
    if invocation.action_name != RESPOND_TO_MESSAGE_ACTION {
        return validate_feedback_record(invocation.input.clone());
    }
    let messages = invocation
        .thread
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol_error("feedback invocation has no conversation messages"))?;
    let source_message_ref = invocation
        .input
        .get("source_message_ref")
        .and_then(Value::as_str)
        .ok_or_else(|| protocol_error("feedback invocation has no source message ref"))?;
    let source_index = messages
        .iter()
        .position(|message| message["message_ref"] == source_message_ref)
        .ok_or_else(|| protocol_error("feedback source message is missing"))?;
    let source = &messages[source_index];
    let text = source["text"]
        .as_str()
        .ok_or_else(|| protocol_error("feedback source message has no text"))?;
    let (score, analysis) = parse_feedback_text(text)?;
    let feedback_actor_ref = source["addressed_actor_refs"]
        .as_array()
        .and_then(|refs| refs.first())
        .and_then(Value::as_str);
    let target_ref = explicit_message_ref(text, messages).or_else(|| {
        messages[..source_index].iter().rev().find_map(|message| {
            let author_is_feedback = feedback_actor_ref
                .is_some_and(|actor_ref| message["author_actor_ref"] == actor_ref);
            let addresses_feedback = feedback_actor_ref.is_some_and(|actor_ref| {
                message["addressed_actor_refs"]
                    .as_array()
                    .is_some_and(|refs| refs.iter().any(|reference| reference == actor_ref))
            });
            if author_is_feedback || addresses_feedback {
                return None;
            }
            message["reply_to_message_ref"]
                .as_str()
                .or_else(|| message["message_ref"].as_str())
                .map(str::to_owned)
        })
    });
    let target_ref = target_ref.ok_or_else(|| {
        protocol_error("feedback needs an earlier non-feedback message or an explicit message ref")
    })?;
    let record = json!({"ref": target_ref, "score": score, "analysis": analysis});
    Ok(json!({"text": serde_json::to_string(&record)?}))
}

fn validate_feedback_record(record: Value) -> CwrResult<Value> {
    let reference = record.get("ref").and_then(Value::as_str);
    let score = record.get("score").and_then(Value::as_i64);
    let analysis = record.get("analysis").and_then(Value::as_str);
    if reference.is_none() || !matches!(score, Some(-3 | -2 | -1 | 1 | 2 | 3)) {
        return protocol_failure("feedback requires ref and a score from -3 to +3, excluding 0");
    }
    if analysis.is_none_or(|analysis| analysis.trim().is_empty()) {
        return protocol_failure("feedback analysis must not be blank");
    }
    Ok(record)
}

fn parse_feedback_text(text: &str) -> CwrResult<(i64, String)> {
    let score = text
        .split_ascii_whitespace()
        .enumerate()
        .find_map(|(index, token)| {
            let trimmed = token.trim_matches(|character: char| {
                matches!(character, ',' | '.' | ':' | ';' | '(' | ')' | '[' | ']')
            });
            let score = trimmed.parse::<i64>().ok()?;
            matches!(score, -3 | -2 | -1 | 1 | 2 | 3).then_some((index, score))
        })
        .ok_or_else(|| protocol_error("feedback needs a score from -3 to +3, excluding 0"))?;
    let words = text.split_ascii_whitespace().collect::<Vec<_>>();
    let mut analysis = words[score.0 + 1..].join(" ");
    if analysis
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("because"))
    {
        analysis = analysis[7..].trim_start_matches([' ', ':', ',']).into();
    }
    if analysis.trim().is_empty() {
        return protocol_failure("feedback needs analysis after the score");
    }
    Ok((score.1, analysis))
}

fn explicit_message_ref(text: &str, messages: &[Value]) -> Option<String> {
    messages.iter().find_map(|message| {
        let reference = message["message_ref"].as_str()?;
        text.contains(reference).then(|| reference.to_owned())
    })
}

#[cfg(test)]
mod loopback_tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn noncommentary_adapter_keeps_explicit_progress_and_terminal_sequence() {
        let mut invocation = AdapterInvocation::new(
            "cwp://com.example.runtime/invocations/loopback-progress",
            "echo--input",
            json!({"message": "hello"}),
            json!({"type": "object"}),
        );
        invocation.model_run_ref =
            Some("cwp://com.example.runtime/model_runs/loopback-progress".into());
        invocation.integration_ref = Some("~@/integrations/loopback".into());
        let invoke = AdapterHostMessage::Invoke {
            protocol: PROCESS_ADAPTER_PROTOCOL_REF.into(),
            invocation: Box::new(invocation),
        };
        let input = format!("{}\n", serde_json::to_string(&invoke).unwrap());
        let mut output = Vec::new();

        run_loopback_adapter(Cursor::new(input), &mut output).unwrap();

        let events = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<AdapterEvent>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(
            &events[0],
            AdapterEvent::Progress {
                sequence: 1,
                status: 102,
                summary,
                ..
            } if summary == "Looping back the action input."
        ));
        assert!(matches!(
            &events[1],
            AdapterEvent::Result {
                sequence: 2,
                status: 200,
                ..
            }
        ));
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn sequential_calls_validate_each_id_and_return_exact_results() {
        let (input, expected) = sequential_input("loopback-call-2");
        let mut output = Vec::new();
        run_loopback_adapter(Cursor::new(input), &mut output).unwrap();
        let events = String::from_utf8(output).unwrap().lines()
            .map(|line| serde_json::from_str::<AdapterEvent>(line).unwrap())
            .collect::<Vec<_>>();
        let ids = events.iter().filter_map(|event| match event {
            AdapterEvent::Call { call_id, .. } => Some(call_id.as_str()), _ => None,
        }).collect::<Vec<_>>();
        assert_eq!(ids, ["loopback-call-1", "loopback-call-2"]);
        match events.last().unwrap() {
            AdapterEvent::Result { result: Some(result), status: 200, .. } => {
                assert_eq!(result["calls"], expected);
                assert_eq!(result["adapter_pid"], std::process::id());
            }
            event => panic!("unexpected result: {event:?}"),
        }
    }

    #[test]
    fn sequential_calls_reject_a_reused_response_id() {
        let (input, _) = sequential_input("loopback-call-1");
        let mut output = Vec::new();
        let error = run_loopback_adapter(Cursor::new(input), &mut output).unwrap_err();
        assert!(error.to_string().contains("wrong host call result"));
        assert!(!String::from_utf8(output).unwrap().contains("\"type\":\"result\""));
    }

    fn sequential_input(second_id: &str) -> (String, Value) {
        let reference = "cwp://com.example.runtime/invocations/sequential";
        let mut invocation = AdapterInvocation::new(reference, "echo--input", json!({}), json!({"type": "object"}));
        invocation.adapter_params = json!({"host_calls": [
            {"operation": "cwr.list_actions", "input": {}},
            {"operation": "cwr.list_coworkers", "input": {}}
        ]});
        let invoke = AdapterHostMessage::Invoke {
            protocol: PROCESS_ADAPTER_PROTOCOL_REF.into(), invocation: Box::new(invocation),
        };
        let mut input = format!("{}\n", serde_json::to_string(&invoke).unwrap());
        let values = [json!({"text": "λ🌱", "value": 1}), json!({"value": 2})];
        for (id, value) in ["loopback-call-1", second_id].into_iter().zip(&values) {
            let reply = AdapterHostMessage::CallResult {
                invocation_ref: reference.into(), call_id: id.into(), result: Some(value.clone()), error: None,
            };
            input.push_str(&format!("{}\n", serde_json::to_string(&reply).unwrap()));
        }
        (input, json!([
            {"call_id": "loopback-call-1", "result": values[0]},
            {"call_id": "loopback-call-2", "result": values[1]}
        ]))
    }

    #[test]
    fn configured_message_response_returns_text_without_a_provider() {
        let mut invocation = AdapterInvocation::new(
            "cwp://com.example.runtime/invocations/welcome",
            RESPOND_TO_MESSAGE_ACTION,
            json!({"source_message_ref": "cwp://com.example.runtime/messages/welcome"}),
            json!({"type": "object"}),
        );
        invocation.adapter_params = json!({"response_text": "Welcome to Workloop."});
        let invoke = AdapterHostMessage::Invoke {
            protocol: PROCESS_ADAPTER_PROTOCOL_REF.into(),
            invocation: Box::new(invocation),
        };
        let input = format!("{}\n", serde_json::to_string(&invoke).unwrap());
        let mut output = Vec::new();

        run_loopback_adapter(Cursor::new(input), &mut output).unwrap();

        let result = String::from_utf8(output)
            .unwrap()
            .lines()
            .last()
            .map(|line| serde_json::from_str::<AdapterEvent>(line).unwrap())
            .unwrap();
        assert!(matches!(
            result,
            AdapterEvent::Result {
                result: Some(value),
                status: 200,
                ..
            } if value == json!({"text": "Welcome to Workloop."})
        ));
    }

    #[test]
    fn feedback_mode_records_the_execution_source_message() {
        let invocation = feedback_invocation(
            "the last item was screwy. -3 because it wandered through random paths",
        );
        assert_eq!(
            feedback_result(&invocation).unwrap(),
            json!({"text": "{\"analysis\":\"it wandered through random paths\",\"ref\":\"cwp://com.example.runtime/messages/request\",\"score\":-3}"})
        );
    }

    #[test]
    fn feedback_mode_rejects_zero() {
        let invocation = feedback_invocation("0 because this cannot be graded");
        assert!(feedback_result(&invocation)
            .unwrap_err()
            .to_string()
            .contains("excluding 0"));
    }

    fn feedback_invocation(text: &str) -> AdapterInvocation {
        let mut invocation = AdapterInvocation::new(
            "cwp://com.example.runtime/invocations/feedback",
            RESPOND_TO_MESSAGE_ACTION,
            json!({"source_message_ref": "cwp://com.example.runtime/messages/feedback"}),
            json!({"type": "object"}),
        );
        invocation.thread = json!({
            "messages": [
                {
                    "message_ref": "cwp://com.example.runtime/messages/request",
                    "author_actor_ref": "~@/actors/randall",
                    "addressed_actor_refs": ["~@/actors/sourceng_ops"],
                    "text": "integrate this"
                },
                {
                    "message_ref": "cwp://com.example.runtime/messages/reply",
                    "author_actor_ref": "~@/actors/sourceng_ops",
                    "addressed_actor_refs": ["~@/actors/randall"],
                    "reply_to_message_ref": "cwp://com.example.runtime/messages/request",
                    "text": "Done"
                },
                {
                    "message_ref": "cwp://com.example.runtime/messages/feedback",
                    "author_actor_ref": "~@/actors/randall",
                    "addressed_actor_refs": ["~@/actors/feedback"],
                    "text": text
                }
            ]
        });
        invocation
    }
}
