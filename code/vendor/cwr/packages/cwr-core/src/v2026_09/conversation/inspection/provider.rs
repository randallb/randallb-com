use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const PROVIDER_DIAGNOSTIC_EVENTS: [&str; 3] = [
    "provider_request_failed",
    "provider_request_recovered",
    "provider_request_retry_scheduled",
];
pub(super) const MAX_PROVIDER_DIAGNOSTICS: usize = 100;
const MAX_DIAGNOSTIC_LINE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderRequestDiagnostic {
    pub event: String,
    pub sequence: u64,
    pub occurred_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_delay_capped: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<ProviderRequestRateLimit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderRequestRateLimit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderTurnDiagnostic {
    pub event: String,
    pub sequence: u64,
    pub occurred_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

pub(super) fn diagnostics_from_provider_event(
    data: &Value,
    sequence: u64,
    occurred_at_unix_ms: u64,
) -> Vec<ProviderRequestDiagnostic> {
    let item = value_object(data.get("item"));
    if item.get("type").and_then(Value::as_str) != Some("commandExecution") {
        return Vec::new();
    }
    let Some(output) = item.get("aggregatedOutput").and_then(Value::as_str) else {
        return Vec::new();
    };
    output
        .lines()
        .filter(|line| line.len() <= MAX_DIAGNOSTIC_LINE_BYTES)
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| safe_provider_diagnostic(&value, sequence, occurred_at_unix_ms))
        .take(MAX_PROVIDER_DIAGNOSTICS)
        .collect()
}

fn safe_provider_diagnostic(
    value: &Value,
    sequence: u64,
    occurred_at_unix_ms: u64,
) -> Option<ProviderRequestDiagnostic> {
    let value = value_object(Some(value));
    let event = value.get("event")?.as_str()?;
    if !PROVIDER_DIAGNOSTIC_EVENTS.contains(&event) {
        return None;
    }
    let rate_limit = value
        .get("rate_limit")
        .and_then(Value::as_object)
        .map(|rate_limit| ProviderRequestRateLimit {
            limit: safe_scalar(rate_limit, "limit", 200),
            remaining: safe_scalar(rate_limit, "remaining", 200),
            reset: safe_scalar(rate_limit, "reset", 200),
        })
        .filter(|value| {
            value.limit.is_some() || value.remaining.is_some() || value.reset.is_some()
        });
    Some(ProviderRequestDiagnostic {
        event: event.into(),
        sequence,
        occurred_at_unix_ms,
        provider: safe_token(value, "provider", 200),
        operation: safe_token(value, "operation", 200),
        attempt: safe_u64(value, "attempt"),
        max_attempts: safe_u64(value, "max_attempts"),
        status: safe_u16(value, "status"),
        elapsed_ms: safe_u64(value, "elapsed_ms"),
        retryable: value.get("retryable").and_then(Value::as_bool),
        retry_delay_ms: safe_u64(value, "retry_delay_ms"),
        retry_delay_capped: value.get("retry_delay_capped").and_then(Value::as_bool),
        request_id: safe_token(value, "request_id", 200),
        retry_after: safe_scalar(value, "retry_after", 200),
        error_kind: safe_token(value, "error_kind", 200),
        rate_limit,
    })
}

pub(super) fn collect_error_code(data: &Value, error_codes: &mut BTreeSet<String>) {
    for terminal_call in [Some(data), data.get("item")] {
        let Some(terminal_call) = terminal_call.and_then(Value::as_object) else {
            continue;
        };
        if terminal_call.get("namespace").and_then(Value::as_str) != Some("cwr")
            || terminal_call.get("tool").and_then(Value::as_str) != Some("return_result")
        {
            continue;
        }
        let Some(code) = terminal_call
            .get("arguments")
            .and_then(|value| value.get("error"))
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str)
            .and_then(|value| sanitized_token(value, 200))
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        error_codes.insert(code);
    }
}

fn safe_token(value: &Map<String, Value>, key: &str, maximum: usize) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| sanitized_token(value, maximum))
}

fn safe_scalar(value: &Map<String, Value>, key: &str, maximum: usize) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| {
            value.chars().all(|character| {
                character.is_ascii_alphanumeric() || " .,:;_+/-".contains(character)
            })
        })
        .map(|value| sanitize_text(value, maximum))
        .filter(|value| !value.is_empty())
}

fn safe_u64(value: &Map<String, Value>, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn safe_u16(value: &Map<String, Value>, key: &str) -> Option<u16> {
    safe_u64(value, key).and_then(|value| value.try_into().ok())
}

fn value_object(value: Option<&Value>) -> &Map<String, Value> {
    match value.and_then(Value::as_object) {
        Some(value) => value,
        None => empty_object(),
    }
}

fn empty_object() -> &'static Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Map::new)
}

fn sanitize_text(value: &str, maximum: usize) -> String {
    let mut sanitized = String::with_capacity(value.len().min(maximum));
    let mut token = String::new();
    let flush = |token: &mut String, sanitized: &mut String| {
        if token.contains('@') && token.rsplit_once('.').is_some() {
            sanitized.push_str("[REDACTED_EMAIL]");
        } else {
            sanitized.push_str(token);
        }
        token.clear();
    };
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || "._%+-@".contains(character) {
            token.push(character);
        } else {
            flush(&mut token, &mut sanitized);
            sanitized.push(character);
        }
        if sanitized.len() >= maximum {
            break;
        }
    }
    flush(&mut token, &mut sanitized);
    if sanitized.len() > maximum {
        let boundary = sanitized
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= maximum)
            .last()
            .unwrap_or(0);
        sanitized.truncate(boundary);
    }
    sanitized
}

pub(super) fn sanitized_token(value: &str, maximum: usize) -> Option<String> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._:/+-".contains(character))
    {
        return None;
    }
    Some(value.chars().take(maximum).collect())
}
