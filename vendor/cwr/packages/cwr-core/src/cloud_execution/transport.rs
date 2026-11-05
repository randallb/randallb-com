use crate::{
    adapter::{
        read_adapter_message, validate_adapter_invocation, write_adapter_message,
        AdapterHostMessage, AdapterInvocation, PROCESS_ADAPTER_PROTOCOL_REF,
    },
    CwrResult,
};
use reqwest::{blocking::Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;
static CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudDispatch {
    pub request_id: String,
    pub workspace_ref: String,
    pub conversation_ref: String,
    pub adapter_ref: String,
    pub route_fingerprint: String,
    pub invocation: AdapterInvocation,
}

/// Returned only to the trusted process proxy. The token is invocation scoped,
/// rather than a reusable provider credential.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudAssignment {
    pub assignment_id: String,
    pub invocation_ref: String,
    pub executor_ref: String,
    pub generation: u64,
    pub expires_at_unix_ms: u64,
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudEvent {
    pub sequence: u64,
    pub frame: Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudEvents {
    pub events: Vec<CloudEvent>,
    pub terminal: bool,
}

extern "C" fn cancel(_: libc::c_int) {
    CANCELLED.store(true, Ordering::SeqCst);
}

fn failure(message: &str) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}

fn required(name: &str) -> CwrResult<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| failure(&format!("Cloud execution requires {name}")))
}

fn base_url(value: &str) -> CwrResult<Url> {
    let url = Url::parse(value)?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(failure(
            "Cloud coordinator requires an HTTPS origin or explicit loopback HTTP origin",
        ));
    }
    Ok(url)
}

fn endpoint(base: &Url, assignment: &CloudAssignment, suffix: &str) -> CwrResult<Url> {
    let mut url = base.clone();
    url.path_segments_mut()
        .map_err(|_| failure("Invalid coordinator origin"))?
        .extend(["v1", "assignments", &assignment.assignment_id, suffix]);
    Ok(url)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn checked_json<T: serde::de::DeserializeOwned>(
    response: reqwest::blocking::Response,
) -> CwrResult<T> {
    if !response.status().is_success() {
        return Err(failure(&format!(
            "Cloud coordinator refused request ({})",
            response.status().as_u16()
        )));
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    response
        .take(MAX_FRAME_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(failure("Cloud response exceeds frame limit"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn send(client: &Client, url: Url, token: &str, body: &Value) -> CwrResult<Value> {
    // A network retry keeps the same request identity and byte-equivalent body.
    // The durable coordinator must return a receipt, never re-execute it.
    for attempt in 0..3 {
        let response = client
            .post(url.clone())
            .bearer_auth(token)
            .json(body)
            .send();
        match response {
            Ok(response) if !response.status().is_server_error() => return checked_json(response),
            Ok(_) | Err(_) if attempt < 2 => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                return Err(failure(
                    "Cloud transport failed; outcome must be reconciled",
                ))
            }
        }
    }
    unreachable!()
}

fn event_transport_error(error: reqwest::Error) -> Box<dyn std::error::Error + Send + Sync> {
    let error = error.without_url();
    let mut message = format!(
        "Cloud event transport failed (timeout={}, connect={})",
        error.is_timeout(),
        error.is_connect()
    );
    let mut cause: Option<&dyn std::error::Error> = Some(&error);
    for _ in 0..4 {
        let Some(error) = cause else {
            break;
        };
        message.push_str(": ");
        message.extend(error.to_string().chars().take(512));
        cause = error.source();
    }
    failure(&message)
}

// Retrying this read reuses the accepted assignment and exact event cursor.
// It never redispatches an invocation or acknowledges an unforwarded frame.
fn read_events(client: &Client, url: Url, assignment: &CloudAssignment) -> CwrResult<CloudEvents> {
    for attempt in 0..3 {
        if CANCELLED.load(Ordering::SeqCst) {
            return Err(failure("Cloud invocation cancellation requested"));
        }
        let remaining = assignment.expires_at_unix_ms.saturating_sub(now());
        if remaining == 0 {
            return Err(failure(
                "Cloud assignment expired; retain interrupted outcome",
            ));
        }
        let response = client
            .get(url.clone())
            .bearer_auth(&assignment.token)
            .timeout(Duration::from_millis(remaining.min(3000)))
            .send();
        match response {
            Ok(response) if response.status().is_server_error() && attempt < 2 => {}
            Ok(response) => match checked_json(response) {
                Ok(batch) => return Ok(batch),
                Err(error)
                    if attempt < 2
                        && error
                            .downcast_ref::<io::Error>()
                            .is_some_and(|e| e.kind() != io::ErrorKind::InvalidData) => {}
                Err(error) => return Err(error),
            },
            Err(_) if attempt < 2 => {}
            Err(error) => return Err(event_transport_error(error)),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    unreachable!()
}

/// Run as a trusted host-side process adapter. Every frame still passes the
/// ordinary CWR process transport's output schema, sequence and host-call checks.
/// The disposable executor has neither this coordinator credential nor CWR state.
pub fn run_proxy<R: BufRead + Send + 'static>(
    mut input: R,
    mut output: impl Write,
) -> CwrResult<()> {
    CANCELLED.store(false, Ordering::SeqCst);
    // SAFETY: handlers only set an atomic flag; no allocation or I/O occurs.
    unsafe {
        if libc::signal(libc::SIGTERM, cancel as *const () as libc::sighandler_t) == libc::SIG_ERR
            || libc::signal(libc::SIGINT, cancel as *const () as libc::sighandler_t)
                == libc::SIG_ERR
        {
            return Err(io::Error::last_os_error().into());
        }
    }
    let first = read_adapter_message::<AdapterHostMessage>(&mut input)?
        .ok_or_else(|| failure("Missing cloud invocation"))?;
    let AdapterHostMessage::Invoke {
        protocol,
        invocation,
    } = first
    else {
        return Err(failure("Cloud adapter requires invoke first"));
    };
    if protocol != PROCESS_ADAPTER_PROTOCOL_REF {
        return Err(failure("Unsupported process protocol"));
    }
    validate_adapter_invocation(&invocation)?;
    let base = base_url(&required("CWR_CLOUD_COORDINATOR_URL")?)?;
    let coordinator_token = required("CWR_CLOUD_COORDINATOR_TOKEN")?;
    let dispatch = CloudDispatch {
        request_id: invocation.invocation_ref.clone(),
        workspace_ref: required("CWR_CLOUD_WORKSPACE_REF")?,
        conversation_ref: invocation.thread["conversation_ref"]
            .as_str()
            .ok_or_else(|| failure("Cloud invocation requires retained conversation context"))?
            .to_owned(),
        adapter_ref: required("CWR_CLOUD_ADAPTER_REF")?,
        route_fingerprint: required("CWR_CLOUD_ROUTE_FINGERPRINT")?,
        invocation: *invocation,
    };
    let client = Client::builder()
        .timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let value = send(
        &client,
        base.join("v1/dispatch")?,
        &coordinator_token,
        &serde_json::to_value(&dispatch)?,
    )?;
    let assignment: CloudAssignment = serde_json::from_value(value)?;
    if assignment.invocation_ref != dispatch.request_id
        || assignment.generation == 0
        || assignment.expires_at_unix_ms <= now()
        || assignment.token.is_empty()
        || assignment.assignment_id.is_empty()
        || assignment.assignment_id.len() > 128
        || !assignment
            .assignment_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(failure("Invalid cloud assignment"));
    }
    let (sender, receiver) = mpsc::sync_channel(2);
    std::thread::spawn(move || loop {
        let frame = read_adapter_message::<AdapterHostMessage>(&mut input);
        let done = !matches!(&frame, Ok(Some(_)));
        if sender.send(frame).is_err() || done {
            break;
        }
    });
    let outcome = forward(
        &client,
        &base,
        &assignment,
        &dispatch.request_id,
        &receiver,
        &mut output,
    );
    let suffix = if CANCELLED.load(Ordering::SeqCst) {
        "cancel"
    } else {
        "interrupt"
    };
    if outcome.is_err() || CANCELLED.load(Ordering::SeqCst) {
        // A failed acknowledgement is uncertainty, not successful cancellation.
        let _ = send(
            &client,
            endpoint(&base, &assignment, suffix)?,
            &assignment.token,
            &json!({"request_id":format!("{}:{suffix}", assignment.assignment_id)}),
        );
    }
    outcome
}

fn forward(
    client: &Client,
    base: &Url,
    assignment: &CloudAssignment,
    invocation_ref: &str,
    input: &mpsc::Receiver<CwrResult<Option<AdapterHostMessage>>>,
    output: &mut impl Write,
) -> CwrResult<()> {
    let mut after = 0u64;
    let mut idle_ms = 25;
    let mut pending = None;
    loop {
        if CANCELLED.load(Ordering::SeqCst) {
            return Err(failure("Cloud invocation cancellation requested"));
        }
        if now() >= assignment.expires_at_unix_ms {
            return Err(failure(
                "Cloud assignment expired; retain interrupted outcome",
            ));
        }
        while let Some(frame) = pending.take().or_else(|| input.try_recv().ok()) {
            idle_ms = 25;
            let frame = frame?.ok_or_else(|| failure("Coordinator process input closed"))?;
            let id = match &frame {
                AdapterHostMessage::CallResult {
                    invocation_ref: caller,
                    call_id,
                    ..
                } if caller == invocation_ref => format!("call:{call_id}"),
                AdapterHostMessage::ActivityResult {
                    invocation_ref: caller,
                    sequence,
                    ..
                } if caller == invocation_ref => format!("activity:{sequence}"),
                _ => return Err(failure("Foreign or repeated cloud invocation input")),
            };
            send(
                client,
                endpoint(base, assignment, "input")?,
                &assignment.token,
                &json!({"request_id":id,"frame":frame}),
            )?;
        }
        let mut url = endpoint(base, assignment, "events")?;
        url.query_pairs_mut()
            .append_pair("after", &after.to_string());
        let batch = read_events(client, url, assignment)?;
        if batch.events.len() > 256 {
            return Err(failure("Cloud event batch exceeds limit"));
        }
        let had_events = !batch.events.is_empty();
        for event in batch.events {
            if event.sequence
                != after
                    .checked_add(1)
                    .ok_or_else(|| failure("Cloud event sequence overflow"))?
                || event.frame["invocation_ref"] != invocation_ref
            {
                return Err(failure("Cloud event sequence or invocation mismatch"));
            }
            write_adapter_message(output, &event.frame)?;
            after = event.sequence;
        }
        if batch.terminal {
            return Ok(());
        }
        if had_events {
            idle_ms = 25;
        } else {
            pending = wait_for_poll_input(input, assignment, Duration::from_millis(idle_ms))?;
            idle_ms = (idle_ms * 2).min(200);
        }
    }
}

// Empty polls back off without delaying host-call replies or stop/expiry checks.
fn wait_for_poll_input(
    input: &mpsc::Receiver<CwrResult<Option<AdapterHostMessage>>>,
    assignment: &CloudAssignment,
    pause: Duration,
) -> CwrResult<Option<CwrResult<Option<AdapterHostMessage>>>> {
    let deadline = std::time::Instant::now() + pause;
    loop {
        if CANCELLED.load(Ordering::SeqCst) {
            return Err(failure("Cloud invocation cancellation requested"));
        }
        let remaining = assignment.expires_at_unix_ms.saturating_sub(now());
        if remaining == 0 {
            return Err(failure(
                "Cloud assignment expired; retain interrupted outcome",
            ));
        }
        let wait = deadline
            .saturating_duration_since(std::time::Instant::now())
            .min(Duration::from_millis(remaining))
            .min(Duration::from_millis(25));
        if wait.is_zero() {
            return Ok(None);
        }
        match input.recv_timeout(wait) {
            Ok(frame) => return Ok(Some(frame)),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(failure("Coordinator process input closed"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coordinator_origin_does_not_export_bearer_credentials() {
        for value in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com/path",
            "https://example.com/?key=x",
            "https://example.com/#fragment",
        ] {
            assert!(base_url(value).is_err(), "{value}");
        }
        assert!(base_url("http://127.0.0.1:8080").is_ok());
        assert!(base_url("https://cloud.example.com").is_ok());
    }
}

#[cfg(test)]
#[path = "transport/event_retry_tests.rs"]
mod event_retry_tests;

#[cfg(test)]
#[path = "transport/poll_input_tests.rs"]
mod poll_input_tests;
