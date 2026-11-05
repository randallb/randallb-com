use super::error;
use crate::{
    adapter::{
        write_adapter_message, AdapterCompletedCommentary, AdapterEvent, AdapterModelOutput,
    },
    CwrResult,
};
use cwr::open_responses::v2026_04_24::{validate_event, validate_output_item, AssistantPhase};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
};

use cwr::adapter::MAX_PROTOCOL_LINE_BYTES;

pub(super) struct ParsedResponse {
    pub id: String,
    pub status: String,
    pub raw_output: Vec<Value>,
    pub messages: Vec<AssistantMessage>,
    pub calls: Vec<FunctionCall>,
    pub error: Option<Value>,
}

pub(super) struct AssistantMessage {
    pub id: String,
    pub text: String,
}

pub(super) struct FunctionCall {
    pub id: String,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

struct MessagePosition {
    output_index: usize,
    first_sequence: i64,
}

include!("output/stream.rs");

impl ParsedResponse {
    pub fn parse(value: &Value) -> CwrResult<Self> {
        let id = required_nonblank(value, "id", "response")?.to_owned();
        let status = required_nonblank(value, "status", "response")?.to_owned();
        let raw_output = value
            .get("output")
            .and_then(Value::as_array)
            .ok_or_else(|| error("Open Responses response has no output array"))?
            .clone();
        let mut messages = Vec::new();
        let mut calls = Vec::new();
        for item in &raw_output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => messages.push(parse_message(item)?),
                Some("function_call") => calls.push(parse_call(item)?),
                _ => {}
            }
        }
        Ok(Self {
            id,
            status,
            raw_output,
            messages,
            calls,
            error: value.get("error").filter(|error| !error.is_null()).cloned(),
        })
    }

    pub fn final_text(&self, value: &Value) -> Option<String> {
        let text = self
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            return Some(text);
        }
        value
            .get("output_text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    }
}

pub(super) fn emit_final_messages(
    output: &mut impl Write,
    invocation_ref: &str,
    model_run_ref: Option<&str>,
    response_id: &str,
    messages: &[AssistantMessage],
    fallback_text: Option<&str>,
) -> CwrResult<()> {
    let Some(model_run_ref) = model_run_ref else {
        return Ok(());
    };
    if messages.is_empty() {
        if let Some(text) = fallback_text {
            return emit_message(
                output,
                invocation_ref,
                model_run_ref,
                response_id,
                &format!("msg_{response_id}"),
                text,
                MessagePosition {
                    output_index: 0,
                    first_sequence: 0,
                },
            );
        }
        return Ok(());
    }
    for (output_index, message) in messages.iter().enumerate() {
        emit_message(
            output,
            invocation_ref,
            model_run_ref,
            response_id,
            &message.id,
            &message.text,
            MessagePosition {
                output_index,
                first_sequence: i64::try_from(output_index)
                    .map_err(|_| error("Open Responses returned too many assistant messages"))?
                    * 5,
            },
        )?;
    }
    Ok(())
}

fn parse_message(item: &Value) -> CwrResult<AssistantMessage> {
    if item.get("role").and_then(Value::as_str) != Some("assistant") {
        return Err(error(
            "Open Responses output message is not from the assistant",
        ));
    }
    let id = required_nonblank(item, "id", "assistant message")?.to_owned();
    let parts = item
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| error("Open Responses assistant message has no content array"))?;
    let text = parts
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    let canonical = json!({
        "type": "message",
        "id": id,
        "status": "completed",
        "role": "assistant",
        "phase": "final_answer",
        "content": [{"type":"output_text","text":text,"annotations":[]}],
    });
    validate_output_item(&canonical).map_err(|cause| {
        error(format!(
            "Open Responses assistant output is invalid: {cause}"
        ))
    })?;
    Ok(AssistantMessage { id, text })
}

fn parse_call(item: &Value) -> CwrResult<FunctionCall> {
    let call = FunctionCall {
        id: required_nonblank(item, "id", "function call")?.to_owned(),
        call_id: required_nonblank(item, "call_id", "function call")?.to_owned(),
        name: required_nonblank(item, "name", "function call")?.to_owned(),
        arguments: item
            .get("arguments")
            .and_then(Value::as_str)
            .ok_or_else(|| error("Open Responses function call has no arguments string"))?
            .to_owned(),
    };
    let canonical = json!({
        "type": "function_call",
        "id": call.id,
        "call_id": call.call_id,
        "name": call.name,
        "arguments": call.arguments,
        "status": "completed",
    });
    validate_output_item(&canonical)
        .map_err(|cause| error(format!("Open Responses function call is invalid: {cause}")))?;
    Ok(call)
}

fn emit_message(
    output: &mut impl Write,
    invocation_ref: &str,
    model_run_ref: &str,
    response_id: &str,
    item_id: &str,
    text: &str,
    position: MessagePosition,
) -> CwrResult<()> {
    let MessagePosition {
        output_index,
        first_sequence,
    } = position;
    let item = json!({
        "type":"message",
        "id":item_id,
        "status":"completed",
        "role":"assistant",
        "phase":"final_answer",
        "content":[{"type":"output_text","text":text,"annotations":[]}],
    });
    let events = [
        json!({
            "type":"response.output_item.added","sequence_number":first_sequence,
            "output_index":output_index,"item":{
                "type":"message","id":item_id,"status":"in_progress",
                "role":"assistant","phase":"final_answer","content":[]
            }
        }),
        json!({
            "type":"response.content_part.added","sequence_number":first_sequence + 1,
            "item_id":item_id,"output_index":output_index,"content_index":0,
            "part":{"type":"output_text","text":"","annotations":[]}
        }),
        json!({
            "type":"response.output_text.done","sequence_number":first_sequence + 2,
            "item_id":item_id,"output_index":output_index,"content_index":0,"text":text
        }),
        json!({
            "type":"response.content_part.done","sequence_number":first_sequence + 3,
            "item_id":item_id,"output_index":output_index,"content_index":0,
            "part":{"type":"output_text","text":text,"annotations":[]}
        }),
        json!({
            "type":"response.output_item.done","sequence_number":first_sequence + 4,
            "output_index":output_index,"item":item
        }),
    ];
    let messages = events.map(|event| AdapterEvent::ModelOutput {
        invocation_ref: invocation_ref.into(),
        output: AdapterModelOutput {
            model_run_ref: model_run_ref.into(),
            provider: "open-responses".into(),
            thread_id: None,
            response_or_turn_id: response_id.into(),
            phase: AssistantPhase::FinalAnswer,
            event,
        },
    });
    for message in &messages {
        if serde_json::to_vec(message)?.len() + 1 > MAX_PROTOCOL_LINE_BYTES {
            return Err(error(
                "Open Responses assistant output exceeds the adapter protocol line limit",
            ));
        }
    }
    for message in messages {
        write_adapter_message(output, &message)?;
    }
    Ok(())
}

fn required_nonblank<'a>(value: &'a Value, field: &str, context: &str) -> CwrResult<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| error(format!("Open Responses {context} has no {field}")))
}
