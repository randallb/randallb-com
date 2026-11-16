use crate::CwrResult;
use reqwest::{
    blocking::{Client as HttpClient, Response},
    header::{ACCEPT, CONTENT_TYPE},
    redirect::Policy,
    StatusCode, Url,
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read},
    time::Duration,
};

const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_BODY_PREVIEW_CHARS: usize = 2048;

pub(super) struct Client {
    endpoint: Url,
    api_key: Option<String>,
    http: HttpClient,
}

pub(super) enum ResponseOutcome {
    Success(Value),
    Failure(ProviderFailure),
}

pub(super) struct ProviderFailure {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
}

impl Client {
    pub fn new(endpoint: Url, api_key: Option<String>) -> CwrResult<Self> {
        let http = HttpClient::builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(Policy::none())
            .user_agent("cwr-adapter-open-responses/0.1")
            .build()?;
        Ok(Self {
            endpoint,
            api_key,
            http,
        })
    }

    pub fn create_response(
        &self,
        request: &Value,
        timeout: Duration,
        mut on_event: impl FnMut(&Value) -> CwrResult<()>,
    ) -> CwrResult<ResponseOutcome> {
        if timeout.is_zero() {
            return Ok(ResponseOutcome::Failure(ProviderFailure {
                status: 504,
                code: "provider_timeout".into(),
                message: "Open Responses invocation exceeded its timeout".into(),
                details: None,
            }));
        }
        let mut builder = self
            .http
            .post(self.endpoint.clone())
            .timeout(timeout)
            .header(ACCEPT, "text/event-stream")
            .json(request);
        if let Some(api_key) = &self.api_key {
            builder = builder.bearer_auth(api_key);
        }
        let response = match builder.send() {
            Ok(response) => response,
            Err(cause) => {
                let (status, code) = if cause.is_timeout() {
                    (504, "provider_timeout")
                } else {
                    (502, "provider_transport_failed")
                };
                return Ok(ResponseOutcome::Failure(ProviderFailure {
                    status,
                    code: code.into(),
                    message: format!("Open Responses request failed: {cause}"),
                    details: None,
                }));
            }
        };
        let streams_events = response.status().is_success()
            && response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/event-stream"));
        if streams_events {
            stream_response(response, &mut on_event)
        } else {
            Ok(response_outcome(response))
        }
    }
}

fn stream_response(
    response: Response,
    on_event: &mut impl FnMut(&Value) -> CwrResult<()>,
) -> CwrResult<ResponseOutcome> {
    read_event_stream(BufReader::new(response), on_event)
}

fn read_event_stream(
    mut reader: impl BufRead,
    on_event: &mut impl FnMut(&Value) -> CwrResult<()>,
) -> CwrResult<ResponseOutcome> {
    let mut event_bytes = 0_u64;
    let mut data_lines = String::new();
    loop {
        let mut line = String::new();
        let read = match (&mut reader)
            .take(MAX_RESPONSE_BYTES - event_bytes + 1)
            .read_line(&mut line)
        {
            Ok(read) => read,
            Err(cause) => {
                return Ok(ResponseOutcome::Failure(ProviderFailure {
                    status: 502,
                    code: "provider_response_read_failed".into(),
                    message: format!("Could not read the Open Responses stream: {cause}"),
                    details: None,
                }))
            }
        };
        if read == 0 {
            if let Some(outcome) = dispatch_sse_data(&mut data_lines, on_event)? {
                return Ok(outcome);
            }
            return Ok(ResponseOutcome::Failure(ProviderFailure {
                status: 502,
                code: "incomplete_provider_stream".into(),
                message: "Open Responses stream ended without a terminal response".into(),
                details: None,
            }));
        }
        event_bytes = event_bytes.saturating_add(read as u64);
        if event_bytes > MAX_RESPONSE_BYTES {
            return Ok(ResponseOutcome::Failure(ProviderFailure {
                status: 502,
                code: "provider_response_too_large".into(),
                message: "Open Responses stream event exceeds 2 MiB".into(),
                details: None,
            }));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            event_bytes = 0;
            if let Some(outcome) = dispatch_sse_data(&mut data_lines, on_event)? {
                return Ok(outcome);
            }
        } else if let Some(data) = line.strip_prefix("data:") {
            data_lines.push_str(data.strip_prefix(' ').unwrap_or(data));
            data_lines.push('\n');
        }
    }
}

fn dispatch_sse_data(
    data_lines: &mut String,
    on_event: &mut impl FnMut(&Value) -> CwrResult<()>,
) -> CwrResult<Option<ResponseOutcome>> {
    if data_lines.is_empty() {
        return Ok(None);
    }
    let mut data = std::mem::take(data_lines);
    data.pop(); // Remove the final SSE field separator, preserving embedded newlines.
    if data == "[DONE]" {
        return Ok(None);
    }
    let event: Value = match serde_json::from_str(&data) {
        Ok(event) => event,
        Err(cause) => {
            return Ok(Some(ResponseOutcome::Failure(ProviderFailure {
                status: 502,
                code: "invalid_provider_response".into(),
                message: format!("Open Responses stream event is not valid JSON: {cause}"),
                details: Some(json!({"provider_body_preview": body_preview(data.as_bytes())})),
            })))
        }
    };
    on_event(&event)?;
    match event.get("type").and_then(Value::as_str) {
        Some("response.completed" | "response.failed" | "response.incomplete") => {
            let Some(response) = event.get("response").filter(|value| value.is_object()) else {
                return Ok(Some(ResponseOutcome::Failure(ProviderFailure {
                    status: 502,
                    code: "invalid_provider_response".into(),
                    message: "Open Responses terminal stream event has no response object".into(),
                    details: None,
                })));
            };
            Ok(Some(ResponseOutcome::Success(response.clone())))
        }
        Some("error") => Ok(Some(ResponseOutcome::Failure(stream_error(&event)))),
        _ => Ok(None),
    }
}

fn stream_error(event: &Value) -> ProviderFailure {
    let error = event.get("error").unwrap_or(event);
    ProviderFailure {
        status: 502,
        code: stringish(error.get("code"))
            .or_else(|| stringish(error.get("type")))
            .unwrap_or_else(|| "provider_stream_error".into()),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty())
            .unwrap_or("Open Responses provider stream failed")
            .into(),
        details: Some(json!({"provider_error":error})),
    }
}

fn response_outcome(mut response: Response) -> ResponseOutcome {
    let status = response.status();
    let mut bytes = Vec::new();
    if let Err(cause) = response
        .by_ref()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
    {
        return ResponseOutcome::Failure(ProviderFailure {
            status: 502,
            code: "provider_response_read_failed".into(),
            message: format!("Could not read the Open Responses response: {cause}"),
            details: None,
        });
    }
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return ResponseOutcome::Failure(ProviderFailure {
            status: if status.is_success() {
                502
            } else {
                provider_status(status)
            },
            code: "provider_response_too_large".into(),
            message: "Open Responses response exceeds 2 MiB".into(),
            details: None,
        });
    }
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(cause) => {
            return ResponseOutcome::Failure(ProviderFailure {
                status: if status.is_success() {
                    502
                } else {
                    provider_status(status)
                },
                code: if status.is_success() {
                    "invalid_provider_response".into()
                } else {
                    format!("http_{}", status.as_u16())
                },
                message: if status.is_success() {
                    format!("Open Responses response is not valid JSON: {cause}")
                } else {
                    status
                        .canonical_reason()
                        .unwrap_or("Provider request failed")
                        .into()
                },
                details: Some(json!({"provider_body_preview": body_preview(&bytes)})),
            });
        }
    };
    if status.is_success() {
        return ResponseOutcome::Success(value);
    }
    let error = value.get("error").unwrap_or(&value);
    let code = stringish(error.get("code"))
        .or_else(|| stringish(error.get("type")))
        .unwrap_or_else(|| format!("http_{}", status.as_u16()));
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| {
            status
                .canonical_reason()
                .unwrap_or("Provider request failed")
        })
        .to_owned();
    ResponseOutcome::Failure(ProviderFailure {
        status: provider_status(status),
        code,
        message,
        details: Some(json!({"provider_error": error})),
    })
}

fn body_preview(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .take(MAX_BODY_PREVIEW_CHARS)
        .collect()
}

fn provider_status(status: StatusCode) -> u16 {
    match status.as_u16() {
        400..=498 | 500..=599 => status.as_u16(),
        499 => 502,
        _ => 502,
    }
}

fn stringish(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
