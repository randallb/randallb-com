use serde_json::{json, Value};

pub(super) fn provider_result(event: &Value) -> Value {
    let subtype = event["subtype"].as_str().unwrap_or("unknown");
    let mut parts = vec![format!("Claude provider error ({subtype})")];
    if let Some(status) = event["api_error_status"].as_u64() {
        parts.push(format!("HTTP {status}"));
    }
    if let Some(result) = event["result"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
    {
        parts.push(result.into());
    }
    if let Some(errors) = event["errors"].as_array() {
        parts.extend(
            errors
                .iter()
                .filter_map(Value::as_str)
                .take(8)
                .map(str::to_owned),
        );
    }
    // Only provider error fields belong in diagnostics, never the raw event
    // (which may include output, usage, or other provider metadata).
    let detail = parts.join(": ");
    let mut chars = detail.chars().map(|c| if c.is_control() { ' ' } else { c });
    let mut message: String = chars.by_ref().take(1024).collect();
    if chars.next().is_some() {
        message.push('…');
    }
    json!({
        "status": 502,
        "summary": "Claude could not complete the request.",
        "error": {"code": "claude_provider_error", "message": message}
    })
}
