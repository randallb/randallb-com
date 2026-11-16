//! Exact bounded public-resource facts. Arbitrary invocation input/result stays private.
use super::*;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResourceFacts {
    pub runtime_root_ref: String,
    pub conversation_ref: String,
    pub commit_revision: String,
    pub conversation: ResourceConversationFacts,
    pub resource: ResourceRecordFacts,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResourceConversationFacts {
    pub owner_context_ref: Option<String>,
    pub parent_ref: Option<String>,
    pub sidebar_eligible: bool,
    pub root: bool,
    pub participant_actor_refs: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceRecordFacts {
    Message {
        record: Value,
        streaming: bool,
        occurred_at_unix_ms: Option<i64>,
    },
    Invocation {
        record: Value,
        child_invocation_refs: Vec<String>,
        wait_records: Vec<Value>,
        latest_reply: Option<Value>,
        execution: Option<ResourceExecutionFacts>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResourceExecutionFacts {
    pub generation: u64,
    pub attempt_active: bool,
    pub started_at_unix_ms: Option<u64>,
    pub latest_activity_at_unix_ms: Option<u64>,
    pub latest_phase_event: Option<Value>,
    pub unpublished_progress: Option<String>,
}
/// Explicit public allowlist, used before facts leave the storage backend.
pub(super) fn invocation_fields(record: &Value) -> Value {
    let mut result = serde_json::Map::new();
    for name in [
        "invocation_ref",
        "conversation_ref",
        "requester_actor_ref",
        "target_actor_ref",
        "source_message_ref",
        "parent_invocation_ref",
        "state",
        "admitted_at_unix_ms",
        "started_at_unix_ms",
        "finished_at_unix_ms",
        "stop_requested_at_unix_ms",
    ] {
        result.insert(name.into(), record[name].clone());
    }
    result.insert(
        "stop_settlement_tracked".into(),
        Value::Bool(record["stop_settlement_tracked"].as_bool().unwrap_or(false)),
    );
    let terminal = matches!(
        record["state"].as_str(),
        Some("completed" | "blocked" | "failed" | "stopped")
    );
    let outcome = if terminal {
        let response = record["responses"]
            .as_array()
            .and_then(|r| r.last())
            .unwrap_or(&Value::Null);
        let detail = |name: &str| match response[name].as_object() {
            Some(value) => {
                serde_json::json!({"code":value.get("code"),"message":value.get("message")})
            }
            None => Value::Null,
        };
        serde_json::json!({"status":response["status"],"summary":response["summary"],"error":detail("error"),"cause":detail("cause")})
    } else {
        Value::Null
    };
    result.insert("outcome".into(), outcome);
    Value::Object(result)
}
