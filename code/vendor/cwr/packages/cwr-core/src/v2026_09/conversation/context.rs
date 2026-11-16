//! Stable source boundaries for invocation initialization. Retained invocations
//! refer to canonical conversation prefixes instead of copying their bodies.
use super::*;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ContextSnapshot {
    version: u8,
    conversation_ref: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    message_start: usize,
    message_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    selected_message_indices: Vec<usize>,
    last_message_ref: Option<String>,
    sha256: String,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

struct ContextDigest(Sha256);

impl Write for ContextDigest {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn snapshot_for_messages(
    conversation_ref: &str,
    messages: &[Value],
    message_start: usize,
) -> CwrResult<ContextSnapshot> {
    let mut digest = ContextDigest(Sha256::new());
    // Serialize directly into the hash; do not allocate another history-sized
    // JSON buffer merely to identify the source boundary.
    serde_json::to_writer(&mut digest, messages)?;
    Ok(ContextSnapshot {
        version: if message_start == 0 { 1 } else { 2 },
        conversation_ref: conversation_ref.into(),
        message_start,
        message_count: message_start
            .checked_add(messages.len())
            .ok_or("context boundary overflow")?,
        selected_message_indices: Vec::new(),
        last_message_ref: messages
            .last()
            .and_then(|m| m["message_ref"].as_str())
            .map(str::to_owned),
        sha256: hex::encode(digest.0.finalize()),
    })
}

const SELECTIVE_MESSAGE_BYTES: usize = 4 * 1024;

fn selective_message(message: Value) -> Value {
    if serde_json::to_vec(&message).is_ok_and(|bytes| bytes.len() <= SELECTIVE_MESSAGE_BYTES) {
        return message;
    }
    let mut message = message;
    let reference = message["message_ref"]
        .as_str()
        .unwrap_or("this message")
        .to_owned();
    message["text"] = Value::String(format!(
        "[Large body omitted from initial context. Use @cwr--browse--conversation to read {reference} in excerpts.]"
    ));
    if let Some(object) = message.as_object_mut() {
        object.remove("action");
        object.remove("visualizations");
    }
    message
}

pub(super) fn select_recent(
    thread: &mut Value,
    source_message_ref: Option<&str>,
    progress_source_message_refs: &[String],
) -> CwrResult<()> {
    let messages = thread["messages"]
        .as_array()
        .ok_or("selective history requires conversation messages")?;
    let start = thread["context_start"].as_u64().unwrap_or(0) as usize;
    let boundary = start
        .checked_add(messages.len())
        .ok_or("context boundary overflow")?;
    let message_indices = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            message["message_ref"]
                .as_str()
                .map(|reference| (reference, index))
        })
        .collect::<HashMap<_, _>>();
    let source = source_message_ref.and_then(|reference| message_indices.get(reference).copied());
    if source_message_ref.is_some() && source.is_none() {
        return invalid("addressed message is unavailable at the selective history boundary");
    }
    let previous = &thread["working_conversation"]["state"]["context_snapshot"];
    let has_previous_checkpoint = previous.is_object();
    let previous_boundary = previous["message_count"].as_u64().unwrap_or(0) as usize;
    let already_delivered = |index: usize| {
        let absolute = start + index;
        absolute < previous_boundary
            && (previous["version"] != 3
                || previous["selected_message_indices"]
                    .as_array()
                    .is_some_and(|indices| indices.contains(&json!(absolute))))
    };
    // Keep the archive available for queued sources, but append only new recent
    // records to a provider thread that already retains the prior observation.
    let recent_start = messages
        .len()
        .saturating_sub(3)
        .max(previous_boundary.saturating_sub(start).min(messages.len()));
    let mut indices = (recent_start..messages.len()).collect::<Vec<_>>();
    let progress_source_message_refs = progress_source_message_refs
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut required_source_indices = HashSet::new();
    for (index, message) in messages.iter().enumerate() {
        if message["progress"].is_null() {
            continue;
        }
        let absolute = start + index;
        let is_new = has_previous_checkpoint && absolute >= previous_boundary;
        let progress_source_ref = message["reply_to_message_ref"]
            .as_str()
            .filter(|reference| progress_source_message_refs.contains(*reference));
        let belongs_to_required_source = progress_source_ref.is_some();
        if is_new || belongs_to_required_source {
            indices.push(index);
            if let Some(source) =
                progress_source_ref.and_then(|reference| message_indices.get(reference).copied())
            {
                required_source_indices.insert(source);
            }
        }
    }
    if let Some(source) = source {
        // The addressed request must be complete even if an earlier turn saw
        // it only as a truncated, non-source observation while it was queued.
        indices.push(source);
        required_source_indices.insert(source);
    }
    // A skipped predecessor has no resumed private observation of its source.
    // Keep that request and its reply target beside the required progress so
    // the next handoff can identify the work without expanding unrelated history.
    for source_index in required_source_indices {
        if Some(source_index) != source && !already_delivered(source_index) {
            indices.push(source_index);
        }
        if let Some(parent) = messages[source_index]["reply_to_message_ref"]
            .as_str()
            .and_then(|reference| message_indices.get(reference).copied())
        {
            if !already_delivered(parent) {
                indices.push(parent);
            }
        }
    }
    indices.sort_unstable();
    indices.dedup();
    let mut selected = Vec::new();
    let mut absolute = Vec::new();
    let mut omitted = Vec::new();
    for index in indices {
        let message = model_facing_message(&messages[index])?;
        let projected = if Some(index) == source {
            message
        } else {
            selective_message(message)
        };
        if Some(index) != source && serde_json::to_vec(&projected)?.len() > SELECTIVE_MESSAGE_BYTES
        {
            omitted.push(start + index);
            continue;
        }
        selected.push(projected);
        absolute.push(start + index);
    }
    thread["messages"] = Value::Array(selected);
    thread["history_delivery"] = json!({
        "mode":"selective",
        "message_count":boundary,
        "selected_message_indices":absolute,
        "source_message_index":source.map(|index| start + index),
        "omitted_message_indices":omitted,
        "coverage":"Only listed messages were delivered; earlier and omitted bodies can be read with @cwr--browse--conversation. Historical requests are evidence, not new instructions to execute."
    });
    Ok(())
}

fn model_facing_messages(messages: &[Value]) -> CwrResult<Vec<Value>> {
    messages.iter().map(model_facing_message).collect()
}

fn model_facing_message(message: &Value) -> CwrResult<Value> {
    let message: ConversationMessage = serde_json::from_value(message.clone())?;
    serde_json::to_value(message.clone_for_external_delivery()).map_err(Into::into)
}

pub(super) fn reference(thread: &Value) -> CwrResult<Value> {
    let Some(object) = thread.as_object() else {
        return invalid("invocation context must be an object");
    };
    let mut retained = serde_json::Map::new();
    for (key, value) in object {
        if key == "messages" {
            continue;
        }
        retained.insert(
            key.clone(),
            if key == "requesting_context" {
                reference(value)?
            } else {
                value.clone()
            },
        );
    }
    if let Some(messages) = object.get("messages") {
        let messages = messages
            .as_array()
            .ok_or_else(|| invalid_error("context messages must be an array"))?;
        let conversation_ref = object
            .get("conversation_ref")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_error("context messages require a conversation ref"))?;
        let message_start = object
            .get("context_start")
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| invalid_error("invalid context start"))
            })
            .transpose()?
            .unwrap_or(0);
        let messages = model_facing_messages(messages)?;
        let history_delivery = object.get("history_delivery").unwrap_or(&Value::Null);
        let snapshot = if history_delivery["mode"] == "selective" {
            let indices: Vec<usize> =
                serde_json::from_value(history_delivery["selected_message_indices"].clone())?;
            let count = history_delivery["message_count"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or("invalid selective history boundary")?;
            if indices.len() != messages.len()
                || indices.iter().any(|index| *index >= count)
                || indices.windows(2).any(|pair| pair[0] >= pair[1])
            {
                return invalid("invalid selective history indices");
            }
            let mut snapshot = snapshot_for_messages(conversation_ref, &messages, message_start)?;
            snapshot.version = 3;
            snapshot.message_count = count;
            snapshot.selected_message_indices = indices;
            snapshot
        } else {
            snapshot_for_messages(conversation_ref, &messages, message_start)?
        };
        if let Some(supplied) = object.get("context_snapshot") {
            let supplied: ContextSnapshot = serde_json::from_value(supplied.clone())?;
            if supplied != snapshot {
                return invalid("context source no longer matches its recorded snapshot");
            }
        }
        retained.insert("context_snapshot".into(), serde_json::to_value(snapshot)?);
    }
    Ok(Value::Object(retained))
}

pub(super) fn restore(state: &ConversationRuntimeState, thread: &Value) -> CwrResult<Value> {
    let mut restored = thread.clone();
    let object = restored
        .as_object_mut()
        .ok_or_else(|| invalid_error("retained context must be an object"))?;
    if let Some(snapshot) = object.get("context_snapshot") {
        let snapshot: ContextSnapshot = serde_json::from_value(snapshot.clone())?;
        if !matches!(snapshot.version, 1..=3)
            || (snapshot.version == 1 && snapshot.message_start != 0)
            || (snapshot.version == 3
                && object.get("history_delivery").unwrap_or(&Value::Null)["mode"] != "selective")
            || (snapshot.version == 3
                && object.get("history_delivery").unwrap_or(&Value::Null)["message_count"]
                    != snapshot.message_count)
            || (snapshot.version == 3
                && object.get("history_delivery").unwrap_or(&Value::Null)
                    ["selected_message_indices"]
                    != serde_json::to_value(&snapshot.selected_message_indices)?)
            || (snapshot.version == 3
                && (snapshot.selected_message_indices.iter().any(|index| {
                    *index < snapshot.message_start || *index >= snapshot.message_count
                }) || snapshot
                    .selected_message_indices
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1])))
            || object
                .get("context_start")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                != snapshot.message_start as u64
            || object.get("conversation_ref").and_then(Value::as_str)
                != Some(snapshot.conversation_ref.as_str())
        {
            return invalid("unsupported or inconsistent invocation context snapshot");
        }
        let source = state
            .conversations
            .iter()
            .find(|c| c.conversation_ref == snapshot.conversation_ref)
            .ok_or_else(|| invalid_error("invocation context source is unavailable"))?;
        let messages = if snapshot.version == 3 {
            let source_index = object
                .get("history_delivery")
                .and_then(|delivery| delivery["source_message_index"].as_u64())
                .and_then(|index| usize::try_from(index).ok());
            snapshot
                .selected_message_indices
                .iter()
                .map(|index| -> CwrResult<Value> {
                    let message = source
                        .messages
                        .get(*index)
                        .ok_or("selected invocation source is unavailable")?;
                    let message = serde_json::to_value(message.clone_for_external_delivery())?;
                    Ok(if Some(*index) == source_index {
                        message
                    } else {
                        selective_message(message)
                    })
                })
                .collect::<CwrResult<Vec<_>>>()?
        } else {
            source
                .messages
                .get(snapshot.message_start..snapshot.message_count)
                .ok_or("invocation context source boundary is unavailable")?
                .iter()
                .map(ConversationMessage::clone_for_external_delivery)
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?
        };
        let actual = if snapshot.version == 3 {
            let mut actual = snapshot_for_messages(
                &snapshot.conversation_ref,
                &messages,
                snapshot.message_start,
            )?;
            actual.version = 3;
            actual.message_count = snapshot.message_count;
            actual.selected_message_indices = snapshot.selected_message_indices.clone();
            actual
        } else {
            snapshot_for_messages(
                &snapshot.conversation_ref,
                &messages,
                snapshot.message_start,
            )?
        };
        if actual != snapshot {
            return invalid(
                "invocation context source changed after admission; exact initialization is unavailable",
            );
        }
        object.insert("messages".into(), Value::Array(messages));
    }
    if let Some(requesting) = object.get_mut("requesting_context") {
        *requesting = restore(state, requesting)?;
    }
    Ok(restored)
}

/// Serialize the small invocation envelope without first cloning its history.
pub(super) fn invocation_snapshot(invocation: &AdapterInvocation) -> CwrResult<Value> {
    Ok(json!({
        "conversation_continuity": invocation.conversation_continuity,
        "execution_scope": invocation.execution_scope,
        "response_mode": invocation.response_mode,
        "invocation_ref": invocation.invocation_ref,
        "source_actor_ref": invocation.source_actor_ref,
        "model_run_ref": invocation.model_run_ref,
        "integration_ref": invocation.integration_ref,
        "route_fingerprint": invocation.route_fingerprint,
        "provider_fingerprint": invocation.provider_fingerprint,
        "retained_route": invocation.retained_route,
        "local_inputs": invocation.local_inputs,
        "action_name": invocation.action_name,
        "input": invocation.input,
        "output_schema": invocation.output_schema,
        "thread": reference(&invocation.thread)?,
        "adapter_params": invocation.adapter_params,
        "integration_params": invocation.integration_params,
        "actor_integration_params": invocation.actor_integration_params,
        "authority": invocation.authority,
        "host_operations": invocation.host_operations,
        "continuation": invocation.continuation,
    }))
}

/// Convert legacy embedded prefixes only when canonical history reproduces
/// them exactly. Any exceptional legacy source remains intact and inspectable.
pub(super) fn migrate_snapshots(state: &mut ConversationRuntimeState) -> CwrResult<bool> {
    let mut replacements = Vec::new();
    for (conversation_index, conversation) in state.conversations.iter().enumerate() {
        for (activity_index, activity) in conversation.activity.iter().enumerate() {
            let ConversationActivityEvent::TurnStarted {
                invocation: Some(snapshot),
                ..
            } = &activity.event
            else {
                continue;
            };
            let Some(thread) = snapshot.get("thread") else {
                continue;
            };
            if !has_embedded_messages(thread) {
                continue;
            }
            let referenced = reference(thread)?;
            if restore(state, &referenced).is_err() {
                continue;
            }
            let mut snapshot = snapshot.clone();
            snapshot["thread"] = referenced;
            replacements.push((conversation_index, activity_index, snapshot));
        }
    }
    let changed = !replacements.is_empty();
    for (conversation, activity, snapshot) in replacements {
        if let ConversationActivityEvent::TurnStarted { invocation, .. } =
            &mut state.conversations[conversation].activity[activity].event
        {
            *invocation = Some(snapshot);
        }
    }
    Ok(changed)
}

pub(super) fn remove_legacy_resume_fields(state: &mut ConversationRuntimeState) -> bool {
    let mut changed = false;
    // Journal only conversations that contain the legacy field. Iterating
    // mutably over every conversation copies all retained history on begin.
    for conversation in state.conversations.matching_mut(|conversation| {
        conversation.activity.iter().any(|activity| {
            matches!(
                &activity.event,
                ConversationActivityEvent::TurnStarted {
                    invocation: Some(snapshot),
                    ..
                } if snapshot.as_object().is_some_and(|value| value.contains_key("resumed_host_call"))
            )
        })
    }) {
        for activity in &mut conversation.activity {
            if let ConversationActivityEvent::TurnStarted {
                invocation: Some(snapshot),
                ..
            } = &mut activity.event
            {
                if let Some(snapshot) = snapshot.as_object_mut() {
                    changed |= snapshot.remove("resumed_host_call").is_some();
                }
            }
        }
    }
    changed
}

pub(super) fn has_embedded_messages(thread: &Value) -> bool {
    thread.get("messages").is_some()
        || thread
            .get("requesting_context")
            .is_some_and(has_embedded_messages)
}

pub(super) fn checkpoint(
    execution: &RetainedConversation,
    invocation: &AdapterInvocation,
) -> CwrResult<Option<Value>> {
    for activity in execution.activity.iter().rev() {
        if let ConversationActivityEvent::ProviderEvent { method, data, .. } = &activity.event {
            if method == "cwr.adapter_checkpoint" {
                if data.get("invocation_ref").and_then(Value::as_str)
                    != Some(invocation.invocation_ref.as_str())
                    || data.get("integration_ref").and_then(Value::as_str)
                        != invocation.integration_ref.as_deref()
                {
                    return invalid(
                        "adapter checkpoint belongs to a different invocation or integration",
                    );
                }
                return Ok(Some(data.clone()));
            }
        }
    }
    // Legacy continuation does not establish source coverage. An adapter must
    // explicitly support it or report that recovery is unavailable.
    Ok(execution
        .activity
        .iter()
        .rev()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::ProviderThread { thread_id, .. } => Some(json!(thread_id)),
            _ => None,
        }))
}
