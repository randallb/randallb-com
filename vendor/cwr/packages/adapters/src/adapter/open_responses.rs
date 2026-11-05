//! Provider-neutral HTTP adapter for the Open Responses response-creation API.
mod client;
mod config;
mod output;

use self::{
    client::{Client, ProviderFailure, ResponseOutcome},
    config::Config,
    output::{emit_final_messages, ParsedResponse, StreamProjection},
};
use crate::{
    adapter::{
        read_adapter_message, validate_adapter_invocation, write_adapter_message,
        AdapterActivityEvent, AdapterCompletedCommentary, AdapterError, AdapterEvent,
        AdapterHostMessage, AdapterInvocation, PROCESS_ADAPTER_PROTOCOL_REF,
    },
    CwrResult,
};
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    time::Instant,
};

const RETURN_TOOL: &str = "cwp__cwr--return--result";
const MAX_ERROR_CODE_CHARS: usize = 128;
const MAX_ERROR_DETAILS_BYTES: usize = 16 * 1024;
const MAX_ERROR_MESSAGE_CHARS: usize = 4096;

pub fn run_open_responses_adapter(
    mut input: impl BufRead,
    mut output: impl Write,
) -> CwrResult<()> {
    let message = read_adapter_message(&mut input)?
        .ok_or_else(|| error("Open Responses adapter received no invocation"))?;
    let (protocol, mut invocation) = match message {
        AdapterHostMessage::Invoke {
            protocol,
            invocation,
        } => (protocol, invocation),
        _ => {
            return Err(error(
                "Open Responses adapter requires an invocation as its first message",
            ))
        }
    };
    if protocol != PROCESS_ADAPTER_PROTOCOL_REF {
        return Err(error(format!(
            "Open Responses adapter supports {PROCESS_ADAPTER_PROTOCOL_REF}, not {protocol}"
        )));
    }
    validate_adapter_invocation(&invocation)?;
    let mut config = Config::new(&mut invocation)?;
    let client = Client::new(config.endpoint.clone(), config.api_key.clone())?;
    let deadline = Instant::now() + config.timeout;
    let mut session = Session {
        invocation: &invocation,
        input: &mut input,
        output: &mut output,
        sequence: 1,
        activity_sequence: 1,
    };
    let mut provider_input = prepare_provider_input(&config, &client, deadline)?;
    let action_request = config.observation["action_request"].take();
    config.observation = json!({"action_request":action_request});

    for _ in 0..config.max_turns {
        maintain_provider_input(&mut provider_input, &config, &client, deadline)?;
        let request = json!({
            "model": config.model,
            "instructions": config.instructions,
            "input": provider_input,
            "tools": config.tools,
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "max_output_tokens": config.max_output_tokens,
            "include": ["reasoning.encrypted_content"],
            "store": false,
            "stream": true,
        });
        let remaining = deadline.saturating_duration_since(Instant::now());
        let mut stream = StreamProjection::default();
        let response_value = match client.create_response(&request, remaining, |event| {
            let completed = stream.observe(
                session.output,
                &invocation.invocation_ref,
                invocation.model_run_ref.as_deref(),
                event,
            )?;
            if let Some(commentary) = completed {
                session.completed_commentary(commentary)?;
            }
            Ok(())
        })? {
            ResponseOutcome::Success(value) => value,
            ResponseOutcome::Failure(failure) => return session.provider_failure(failure, None),
        };
        let response = match ParsedResponse::parse(&response_value) {
            Ok(response) => response,
            Err(cause) => {
                return session.provider_failure(
                    ProviderFailure {
                        status: 502,
                        code: "invalid_provider_response".into(),
                        message: cause.to_string(),
                        details: None,
                    },
                    None,
                )
            }
        };
        if response.status != "completed" || response.error.is_some() {
            let error_value = response.error.as_ref();
            let failure = ProviderFailure {
                status: 502,
                code: error_value
                    .and_then(|value| value.get("code"))
                    .and_then(Value::as_str)
                    .unwrap_or("provider_response_failed")
                    .into(),
                message: error_value
                    .and_then(|value| value.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Open Responses provider did not complete the response")
                    .into(),
                details: error_value.cloned(),
            };
            return session.provider_failure(failure, Some(response.id));
        }

        if !response.calls.is_empty() {
            if let Some(return_call) = response.calls.iter().find(|call| call.name == RETURN_TOOL) {
                if config.native_text || response.calls.len() != 1 {
                    return session.invalid_model_output(
                        "cwp__cwr--return--result must be the sole function call for a named action",
                        Some(response.id),
                    );
                }
                let returned: Value = match serde_json::from_str(&return_call.arguments) {
                    Ok(returned) => returned,
                    Err(cause) => {
                        return session.invalid_model_output(
                            format!("cwp__cwr--return--result arguments are invalid JSON: {cause}"),
                            Some(response.id),
                        )
                    }
                };
                if !jsonschema::is_valid(&config.return_schema, &returned) {
                    return session.invalid_model_output(
                        "cwp__cwr--return--result does not match the requested action contract",
                        Some(response.id),
                    );
                }
                return session.return_result(returned);
            }

            let mut outputs = Vec::with_capacity(response.calls.len());
            for call in &response.calls {
                let Some(operation) = config.tool_operations.get(&call.name) else {
                    return session.invalid_model_output(
                        format!("model called undeclared function {}", call.name),
                        Some(response.id),
                    );
                };
                let arguments: Value = match serde_json::from_str(&call.arguments) {
                    Ok(arguments) => arguments,
                    Err(cause) => {
                        return session.invalid_model_output(
                            format!("function {} arguments are invalid JSON: {cause}", call.name),
                            Some(response.id),
                        )
                    }
                };
                let result = session.declared_tool_call(&call.call_id, operation, arguments)?;
                outputs.push(json!({
                    "type": "function_call_output",
                    "call_id": call.call_id,
                    "output": serde_json::to_string(&result)?,
                }));
            }
            provider_input.extend(response.raw_output);
            provider_input.extend(outputs);
            session.progress("Continuing after CWR function calls.")?;
            continue;
        }

        if !config.native_text {
            return session.invalid_model_output(
                "named action completed without cwp__cwr--return--result",
                Some(response.id),
            );
        }
        let Some(text) = response.final_text(&response_value) else {
            return session.invalid_model_output(
                "text action completed without assistant text",
                Some(response.id),
            );
        };
        if !stream.emitted_text() {
            if let Err(cause) = emit_final_messages(
                session.output,
                &invocation.invocation_ref,
                invocation.model_run_ref.as_deref(),
                &response.id,
                &response.messages,
                Some(&text),
            ) {
                return session.invalid_model_output(cause.to_string(), Some(response.id));
            }
        }
        return session.result(
            200,
            "Open Responses model replied.",
            Some(json!({"text":text})),
            None,
        );
    }
    session.provider_failure(
        ProviderFailure {
            status: 502,
            code: "provider_turn_limit".into(),
            message: format!(
                "Open Responses model exceeded the {} response limit",
                config.max_turns
            ),
            details: None,
        },
        None,
    )
}

struct Session<'a, I, O> {
    invocation: &'a AdapterInvocation,
    input: &'a mut I,
    output: &'a mut O,
    sequence: u64,
    activity_sequence: u64,
}

impl<I: BufRead, O: Write> Session<'_, I, O> {
    fn completed_commentary(&mut self, commentary: AdapterCompletedCommentary) -> CwrResult<()> {
        let sequence = self.activity_sequence;
        write_adapter_message(
            self.output,
            &AdapterEvent::Activity {
                invocation_ref: self.invocation.invocation_ref.clone(),
                sequence,
                event: AdapterActivityEvent::CompletedCommentary { commentary },
            },
        )?;
        let response = read_adapter_message(self.input)?
            .ok_or_else(|| error("Open Responses adapter received no activity result"))?;
        let AdapterHostMessage::ActivityResult {
            invocation_ref,
            sequence: response_sequence,
            error: response_error,
        } = response
        else {
            return Err(error(
                "Open Responses adapter received the wrong activity result message",
            ));
        };
        if invocation_ref != self.invocation.invocation_ref || response_sequence != sequence {
            return Err(error(
                "Open Responses adapter received the wrong activity result",
            ));
        }
        if let Some(response_error) = response_error {
            return Err(error(response_error.message.unwrap_or(response_error.code)));
        }
        self.activity_sequence += 1;
        Ok(())
    }

    fn progress(&mut self, summary: &str) -> CwrResult<()> {
        write_adapter_message(
            self.output,
            &AdapterEvent::Progress {
                progress: None,
                published: false,
                invocation_ref: self.invocation.invocation_ref.clone(),
                sequence: self.sequence,
                status: 102,
                summary: summary.into(),
            },
        )?;
        self.sequence += 1;
        Ok(())
    }

    fn declared_tool_call(
        &mut self,
        call_id: &str,
        operation: &str,
        arguments: Value,
    ) -> CwrResult<Value> {
        let definition = self
            .invocation
            .host_operations
            .iter()
            .find(|candidate| candidate.name == operation)
            .ok_or("declared tool has no exact operation mapping")?;
        match crate::adapter::tools::dispatch(definition, arguments) {
            Ok((operation, arguments)) => self.host_call(call_id, &operation, arguments),
            Err(error) => {
                Ok(json!({"error":{"code":"tool_contract_rejected","message":error.to_string()}}))
            }
        }
    }

    fn host_call(&mut self, call_id: &str, operation: &str, input: Value) -> CwrResult<Value> {
        write_adapter_message(
            self.output,
            &AdapterEvent::Call {
                invocation_ref: self.invocation.invocation_ref.clone(),
                call_id: call_id.into(),
                operation: operation.into(),
                input,
            },
        )?;
        let response = read_adapter_message(self.input)?
            .ok_or_else(|| error("Open Responses adapter received no host call result"))?;
        match response {
            AdapterHostMessage::CallResult {
                invocation_ref,
                call_id: returned_id,
                result,
                error: host_error,
            } if invocation_ref == self.invocation.invocation_ref && returned_id == call_id => {
                match (result, host_error) {
                    (Some(result), None) => Ok(json!({"result":result})),
                    (None, Some(host_error)) => Ok(json!({"error":host_error})),
                    _ => Err(error("Open Responses host call result is invalid")),
                }
            }
            _ => Err(error(
                "Open Responses adapter received the wrong host call result",
            )),
        }
    }

    fn return_result(&mut self, returned: Value) -> CwrResult<()> {
        let status = returned["status"]
            .as_u64()
            .and_then(|status| u16::try_from(status).ok())
            .ok_or_else(|| error("Open Responses action result has no valid status"))?;
        let summary = returned["summary"]
            .as_str()
            .ok_or_else(|| error("Open Responses action result has no summary"))?;
        let result = returned.get("result").cloned();
        let response_error = returned
            .get("error")
            .cloned()
            .map(serde_json::from_value::<AdapterError>)
            .transpose()
            .map_err(|cause| {
                error(format!(
                    "Open Responses action result error is invalid: {cause}"
                ))
            })?;
        self.result(status, summary, result, response_error)
    }

    fn invalid_model_output(
        &mut self,
        message: impl Into<String>,
        response_id: Option<String>,
    ) -> CwrResult<()> {
        self.provider_failure(
            ProviderFailure {
                status: 502,
                code: "invalid_model_output".into(),
                message: message.into(),
                details: None,
            },
            response_id,
        )
    }

    fn provider_failure(
        &mut self,
        failure: ProviderFailure,
        response_id: Option<String>,
    ) -> CwrResult<()> {
        let details = match (failure.details, response_id) {
            (Some(mut details @ Value::Object(_)), Some(response_id)) => {
                details["response_id"] = json!(response_id);
                Some(details)
            }
            (Some(details), Some(response_id)) => {
                Some(json!({"provider":details,"response_id":response_id}))
            }
            (details, None) => details,
            (None, Some(response_id)) => Some(json!({"response_id":response_id})),
        }
        .map(bounded_error_details);
        let code = if failure.code.trim().is_empty() {
            "provider_request_failed".into()
        } else {
            failure.code.chars().take(MAX_ERROR_CODE_CHARS).collect()
        };
        self.result(
            failure.status,
            "Open Responses request failed.",
            None,
            Some(AdapterError {
                code,
                message: Some(
                    failure
                        .message
                        .chars()
                        .take(MAX_ERROR_MESSAGE_CHARS)
                        .collect(),
                ),
                details,
            }),
        )
    }

    fn result(
        &mut self,
        status: u16,
        summary: &str,
        result: Option<Value>,
        response_error: Option<AdapterError>,
    ) -> CwrResult<()> {
        write_adapter_message(
            self.output,
            &AdapterEvent::Result {
                invocation_ref: self.invocation.invocation_ref.clone(),
                sequence: self.sequence,
                status,
                summary: summary.into(),
                result,
                error: response_error,
                continuation: None,
            },
        )
    }
}

fn bounded_error_details(details: Value) -> Value {
    match serde_json::to_vec(&details) {
        Ok(encoded) if encoded.len() <= MAX_ERROR_DETAILS_BYTES => details,
        Ok(encoded) => json!({
            "truncated": true,
            "original_bytes": encoded.len(),
        }),
        Err(_) => json!({"truncated": true}),
    }
}

fn error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message.into()).into()
}

fn context_input(records: &[Value]) -> CwrResult<Vec<Value>> {
    records
        .iter()
        .map(|record| {
            Ok(json!({"type":"message","role":"user",
        "content":[{"type":"input_text","text":serde_json::to_string(record)?}]}))
        })
        .collect()
}

fn prepare_provider_input(
    config: &Config,
    client: &Client,
    deadline: Instant,
) -> CwrResult<Vec<Value>> {
    let records = crate::adapter::context::prepare(
        &config.observation,
        config.context_budget_bytes,
        |records| compact_provider_context(config, client, deadline, records),
    )?;
    let mut provider_input = context_input(&records)?;
    append_exact_request(&mut provider_input, config)?;
    Ok(provider_input)
}

fn compact_provider_context(
    config: &Config,
    client: &Client,
    deadline: Instant,
    records: &[Value],
) -> CwrResult<String> {
    let request = json!({"model":config.model,
            "instructions":crate::adapter::context::COMPACTION_INSTRUCTIONS,
            "input":context_input(records)?,"tools":[],"tool_choice":"none",
            "max_output_tokens":4096,"store":false,"stream":true});
    let response = client.create_response(
        &request,
        deadline.saturating_duration_since(Instant::now()),
        |_| Ok(()),
    )?;
    let ResponseOutcome::Success(value) = response else {
        return Err(error(
            "Open Responses context compaction failed before live execution",
        ));
    };
    let parsed = ParsedResponse::parse(&value)?;
    if parsed.status != "completed" || parsed.error.is_some() || !parsed.calls.is_empty() {
        return Err(error(
            "Open Responses context compaction did not complete without tools",
        ));
    }
    parsed
        .final_text(&value)
        .ok_or_else(|| error("Open Responses context compaction returned no summary"))
}
fn append_exact_request(provider_input: &mut Vec<Value>, config: &Config) -> CwrResult<()> {
    provider_input.push(json!({"type":"message","role":"user","content":[{
        "type":"input_text","text":serde_json::to_string(&json!({"action_request":config.observation["action_request"]}))?
    }]}));
    Ok(())
}

fn maintain_provider_input(
    provider_input: &mut Vec<Value>,
    config: &Config,
    client: &Client,
    deadline: Instant,
) -> CwrResult<()> {
    if crate::adapter::context::encoded_bytes(&provider_input)? > config.context_budget_bytes {
        let completed_history = Value::Object(serde_json::Map::from_iter([(
            "messages".into(),
            Value::Array(std::mem::take(provider_input)),
        )]));
        let records = crate::adapter::context::prepare(
            &completed_history,
            config.context_budget_bytes,
            |records| compact_provider_context(config, client, deadline, records),
        )?;
        *provider_input = context_input(&records)?;
        append_exact_request(provider_input, config)?;
        if crate::adapter::context::encoded_bytes(&provider_input)? > config.context_budget_bytes {
            return Err(error("Open Responses compaction left insufficient room; completed host results are preserved"));
        }
    }
    Ok(())
}
