//! Exact Codex assistant-message mapping into the dated Open Responses subset.

use cwr::open_responses::v2026_04_24::{AssistantPhase, StreamValidator};
use cwr::CwrResult;
use serde_json::{json, Value};
use std::{collections::BTreeMap, io};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletedAssistantMessage {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub phase: AssistantPhase,
    pub text: String,
}

#[derive(Debug, Default)]
pub(crate) struct MappedOpenResponses {
    pub events: Vec<MappedOpenResponseEvent>,
    pub completed_assistant: Option<CompletedAssistantMessage>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MappedOpenResponseEvent {
    pub phase: AssistantPhase,
    pub event: Value,
}

#[derive(Debug, Clone)]
struct OpenAssistantItem {
    output_index: u64,
    thread_id: String,
    turn_id: String,
    item_id: String,
    phase: AssistantPhase,
}

#[derive(Debug)]
pub(crate) struct CodexOpenResponsesMapper {
    expected_thread_id: String,
    expected_turn_id: String,
    stream: StreamValidator,
    next_sequence: u64,
    next_output_index: u64,
    items: BTreeMap<(String, String), OpenAssistantItem>,
}

impl CodexOpenResponsesMapper {
    pub fn new(expected_thread_id: impl Into<String>, expected_turn_id: impl Into<String>) -> Self {
        Self {
            expected_thread_id: expected_thread_id.into(),
            expected_turn_id: expected_turn_id.into(),
            stream: StreamValidator::new(),
            next_sequence: 0,
            next_output_index: 0,
            items: BTreeMap::new(),
        }
    }

    pub fn map_message(&mut self, message: &Value) -> CwrResult<MappedOpenResponses> {
        match message.get("method").and_then(Value::as_str) {
            Some("item/started") => self.start_assistant(message),
            Some("item/agentMessage/delta") => self.append_assistant_delta(message),
            Some("item/completed") => self.complete_assistant(message),
            _ => Ok(MappedOpenResponses::default()),
        }
    }

    fn start_assistant(&mut self, message: &Value) -> CwrResult<MappedOpenResponses> {
        let Some(context) = assistant_context(message) else {
            return Ok(MappedOpenResponses::default());
        };
        if !self.matches_scope(context.thread_id, context.turn_id) {
            return Ok(MappedOpenResponses::default());
        }
        let Some(phase) = assistant_phase(context.item.get("phase")) else {
            return Ok(MappedOpenResponses::default());
        };
        let key = (context.turn_id.to_owned(), context.item_id.to_owned());
        if self.items.contains_key(&key) {
            return invalid(format!(
                "Codex assistant item {} started more than once in turn {}",
                context.item_id, context.turn_id
            ));
        }

        let output_index = self.next_output_index;
        self.next_output_index += 1;
        let item = OpenAssistantItem {
            output_index,
            thread_id: context.thread_id.to_owned(),
            turn_id: context.turn_id.to_owned(),
            item_id: context.item_id.to_owned(),
            phase,
        };
        let mut mapped = MappedOpenResponses::default();
        self.emit(
            json!({
                "type": "response.output_item.added",
                "sequence_number": self.next_sequence,
                "output_index": output_index,
                "item": assistant_item(&item, "in_progress", Vec::new()),
            }),
            &mut mapped.events,
        )?;
        self.emit(
            json!({
                "type": "response.content_part.added",
                "sequence_number": self.next_sequence,
                "item_id": context.item_id,
                "output_index": output_index,
                "content_index": 0,
                "part": output_text(""),
            }),
            &mut mapped.events,
        )?;
        if let Some(text) = context.item.get("text").and_then(Value::as_str) {
            if !text.is_empty() {
                self.emit(
                    json!({
                        "type": "response.output_text.delta",
                        "sequence_number": self.next_sequence,
                        "item_id": context.item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "delta": text,
                    }),
                    &mut mapped.events,
                )?;
            }
        }
        self.items.insert(key, item);
        Ok(mapped)
    }

    fn append_assistant_delta(&mut self, message: &Value) -> CwrResult<MappedOpenResponses> {
        let Some(thread_id) = nonblank(message.pointer("/params/threadId").and_then(Value::as_str))
        else {
            return Ok(MappedOpenResponses::default());
        };
        let Some(turn_id) = nonblank(message.pointer("/params/turnId").and_then(Value::as_str))
        else {
            return Ok(MappedOpenResponses::default());
        };
        let Some(item_id) = nonblank(message.pointer("/params/itemId").and_then(Value::as_str))
        else {
            return Ok(MappedOpenResponses::default());
        };
        let Some(delta) = message.pointer("/params/delta").and_then(Value::as_str) else {
            return Ok(MappedOpenResponses::default());
        };
        if !self.matches_scope(thread_id, turn_id) {
            return Ok(MappedOpenResponses::default());
        }
        let Some(item) = self.items.get(&(turn_id.to_owned(), item_id.to_owned())) else {
            return Ok(MappedOpenResponses::default());
        };
        if item.thread_id != thread_id {
            return Ok(MappedOpenResponses::default());
        }
        let output_index = item.output_index;
        let mut mapped = MappedOpenResponses::default();
        self.emit(
            json!({
                "type": "response.output_text.delta",
                "sequence_number": self.next_sequence,
                "item_id": item_id,
                "output_index": output_index,
                "content_index": 0,
                "delta": delta,
            }),
            &mut mapped.events,
        )?;
        Ok(mapped)
    }

    fn complete_assistant(&mut self, message: &Value) -> CwrResult<MappedOpenResponses> {
        let Some(context) = assistant_context(message) else {
            return Ok(MappedOpenResponses::default());
        };
        if !self.matches_scope(context.thread_id, context.turn_id) {
            return Ok(MappedOpenResponses::default());
        }
        let key = (context.turn_id.to_owned(), context.item_id.to_owned());
        let Some(text) = context.item.get("text").and_then(Value::as_str) else {
            return Ok(MappedOpenResponses::default());
        };
        let completed_phase = assistant_phase(context.item.get("phase"));
        if context
            .item
            .get("phase")
            .is_some_and(|phase| !phase.is_null())
            && completed_phase.is_none()
        {
            return Ok(MappedOpenResponses::default());
        }
        let mut mapped = MappedOpenResponses::default();
        let item = if let Some(item) = self.items.get(&key) {
            if item.thread_id != context.thread_id {
                return Ok(mapped);
            }
            if completed_phase.is_some_and(|phase| phase != item.phase) {
                return invalid(format!(
                    "Codex assistant item {} changed phase before completion",
                    context.item_id
                ));
            }
            self.items
                .remove(&key)
                .expect("checked Codex assistant item must remain present")
        } else {
            let Some(phase) = completed_phase else {
                return Ok(mapped);
            };
            let output_index = self.next_output_index;
            self.next_output_index += 1;
            let item = OpenAssistantItem {
                output_index,
                thread_id: context.thread_id.to_owned(),
                turn_id: context.turn_id.to_owned(),
                item_id: context.item_id.to_owned(),
                phase,
            };
            self.emit(
                json!({
                    "type": "response.output_item.added",
                    "sequence_number": self.next_sequence,
                    "output_index": output_index,
                    "item": assistant_item(&item, "in_progress", Vec::new()),
                }),
                &mut mapped.events,
            )?;
            self.emit(
                json!({
                    "type": "response.content_part.added",
                    "sequence_number": self.next_sequence,
                    "item_id": context.item_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "part": output_text(""),
                }),
                &mut mapped.events,
            )?;
            item
        };
        self.emit(
            json!({
                "type": "response.output_text.done",
                "sequence_number": self.next_sequence,
                "item_id": context.item_id,
                "output_index": item.output_index,
                "content_index": 0,
                "text": text,
            }),
            &mut mapped.events,
        )?;
        self.emit(
            json!({
                "type": "response.content_part.done",
                "sequence_number": self.next_sequence,
                "item_id": context.item_id,
                "output_index": item.output_index,
                "content_index": 0,
                "part": output_text(text),
            }),
            &mut mapped.events,
        )?;
        self.emit(
            json!({
                "type": "response.output_item.done",
                "sequence_number": self.next_sequence,
                "output_index": item.output_index,
                "item": assistant_item(&item, "completed", vec![output_text(text)]),
            }),
            &mut mapped.events,
        )?;
        mapped.completed_assistant = Some(CompletedAssistantMessage {
            thread_id: item.thread_id,
            turn_id: item.turn_id,
            item_id: item.item_id,
            phase: item.phase,
            text: text.to_owned(),
        });
        Ok(mapped)
    }

    fn emit(&mut self, event: Value, events: &mut Vec<MappedOpenResponseEvent>) -> CwrResult<()> {
        let validated = self.stream.accept(&event)?;
        let phase = validated
            .phase
            .ok_or_else(|| invalid_error("mapped Codex assistant event has no resolved phase"))?;
        self.next_sequence += 1;
        events.push(MappedOpenResponseEvent { phase, event });
        Ok(())
    }

    pub(crate) fn matches_scope(&self, thread_id: &str, turn_id: &str) -> bool {
        thread_id == self.expected_thread_id && turn_id == self.expected_turn_id
    }

    pub(crate) fn thread_id(&self) -> &str {
        &self.expected_thread_id
    }

    pub(crate) fn turn_id(&self) -> &str {
        &self.expected_turn_id
    }
}

struct AssistantContext<'a> {
    thread_id: &'a str,
    turn_id: &'a str,
    item_id: &'a str,
    item: &'a Value,
}

fn assistant_context(message: &Value) -> Option<AssistantContext<'_>> {
    let item = message.pointer("/params/item")?;
    if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
        return None;
    }
    Some(AssistantContext {
        thread_id: nonblank(message.pointer("/params/threadId")?.as_str())?,
        turn_id: nonblank(message.pointer("/params/turnId")?.as_str())?,
        item_id: nonblank(item.get("id")?.as_str())?,
        item,
    })
}

fn assistant_phase(value: Option<&Value>) -> Option<AssistantPhase> {
    match value.and_then(Value::as_str) {
        Some("commentary") => Some(AssistantPhase::Commentary),
        Some("final_answer") => Some(AssistantPhase::FinalAnswer),
        _ => None,
    }
}

fn assistant_item(item: &OpenAssistantItem, status: &str, content: Vec<Value>) -> Value {
    json!({
        "type": "message",
        "id": item.item_id,
        "status": status,
        "role": "assistant",
        "content": content,
        "phase": match item.phase {
            AssistantPhase::Commentary => "commentary",
            AssistantPhase::FinalAnswer => "final_answer",
        },
    })
}

fn output_text(text: &str) -> Value {
    json!({"type": "output_text", "text": text, "annotations": []})
}

fn nonblank(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(invalid_error(message))
}

fn invalid_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message.into()).into()
}

include!("open_responses/tests.rs");
