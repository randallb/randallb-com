//! Explicit remote schema: local identifiers and arbitrary string data stay local.
use super::{json, MetricRecord, Value};
use sha2::{Digest, Sha256};

fn anonymous(identity: &str, reference: &str) -> String {
    hex::encode(Sha256::digest(format!("{identity}\n{reference}")))
}

pub(super) fn export_properties(record: &MetricRecord, identity: &str) -> Value {
    let context = &record.context;
    let mut properties = json!({
        "schema_version": record.schema_version,
        "app_version": record.app_version,
        "producer": record.producer,
        "checkpoint": record.checkpoint,
        "attempt": context.attempt,
        "context_bytes": context.context_bytes,
        "invocation_continuation_supplied": context.invocation_continuation_supplied,
        "private_history_supplied": context.private_history_supplied,
        "dropped_records_total": record.dropped_records_total,
        "distinct_id": identity,
        "runtime_ref": anonymous(identity, &context.runtime_ref),
        "invocation_ref": anonymous(identity, &context.invocation_ref),
        "measurements": measurements(&record.measurements, identity),
    });
    for (key, reference) in [
        ("conversation_ref", &context.conversation_ref),
        ("parent_invocation_ref", &context.parent_invocation_ref),
        ("model_run_ref", &context.model_run_ref),
    ] {
        if let Some(reference) = reference {
            properties[key] = json!(anonymous(identity, reference));
        }
    }
    properties
}

fn measurements(value: &Value, identity: &str) -> Value {
    let Some(object) = value.as_object() else {
        return Value::Null;
    };
    let mut safe = serde_json::Map::new();
    for (key, value) in object {
        let sanitized = match value {
            Value::Number(_) | Value::Bool(_) | Value::Null => value.clone(),
            Value::Object(_) => measurements(value, identity),
            Value::String(text)
                if matches!(key.as_str(), "conversation_ref" | "source_message_ref") =>
            {
                json!(anonymous(identity, text))
            }
            Value::String(_) if category_allowed(key, value) => value.clone(),
            _ => continue,
        };
        safe.insert(key.clone(), sanitized);
    }
    Value::Object(safe)
}

fn category_allowed(key: &str, value: &Value) -> bool {
    use crate::adapter::{AdapterCompactionOutcome, TimingCheckpoint};
    use crate::v2026_09::invocation::ActionInvocationState;
    let text = value.as_str().unwrap_or_default();
    match key {
        "state" => serde_json::from_value::<ActionInvocationState>(value.clone()).is_ok(),
        "outcome" => {
            serde_json::from_value::<ActionInvocationState>(value.clone()).is_ok()
                || serde_json::from_value::<AdapterCompactionOutcome>(value.clone()).is_ok()
                || matches!(text, "provider_unavailable" | "unknown")
        }
        "checkpoint" => serde_json::from_value::<TimingCheckpoint>(value.clone()).is_ok(),
        "stage" => matches!(
            text,
            "route_selection"
                | "action_only_message"
                | "lifecycle_operations"
                | "invocation_retention"
                | "invocation_started_observer"
                | "adapter_readiness"
        ),
        "phase" => text == "awaiting_result",
        "stdout_state" => matches!(text, "open" | "eof"),
        _ => false,
    }
}
