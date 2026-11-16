//! Native Claude print-mode adapter. Only declared CWR host tools are exposed.
mod config;
mod failure;
pub mod mcp;
mod runtime;
mod session;
mod trace;

use crate::{
    adapter::{
        read_adapter_message, validate_adapter_invocation, AdapterHostMessage,
        PROCESS_ADAPTER_PROTOCOL_REF,
    },
    CwrResult,
};
use serde_json::{json, Value};
use std::{
    io::{self, BufReader},
    os::unix::net::UnixStream,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

enum Input {
    Host(AdapterHostMessage),
    HostClosed,
    Provider(Value),
    ProviderClosed,
    Mcp(Value, UnixStream),
    Failure(String),
}

pub fn run_claude_adapter() -> CwrResult<()> {
    trace::emit("adapter", "starting");
    let outcome = run_claude_adapter_inner();
    match &outcome {
        Ok(()) => trace::emit("adapter", "completed"),
        Err(cause) => trace::emit("adapter", format!("failed cause={cause}")),
    }
    outcome
}

fn run_claude_adapter_inner() -> CwrResult<()> {
    runtime::install_stop_handler()?;
    let (sender, receiver) = mpsc::sync_channel(64);
    let host_sender = sender.clone();
    thread::spawn(move || {
        let outcome = (|| -> CwrResult<()> {
            let mut input = BufReader::new(io::stdin().lock());
            while let Some(message) = read_adapter_message(&mut input)? {
                host_sender.send(Input::Host(message))?;
            }
            host_sender.send(Input::HostClosed)?;
            Ok(())
        })();
        if let Err(cause) = outcome {
            let _ = host_sender.send(Input::Failure(cause.to_string()));
        }
    });
    let first_message = receive_initial_context(&receiver, runtime::stopping)?;
    let mut invocation = match first_message {
        Input::Host(AdapterHostMessage::Invoke {
            protocol,
            invocation,
        }) if protocol == PROCESS_ADAPTER_PROTOCOL_REF => invocation,
        _ => {
            return Err(error(
                "Claude adapter requires a supported invocation as its first message",
            ))
        }
    };
    validate_adapter_invocation(&invocation)?;
    let mut config = config::Config::new(&mut invocation)?;
    trace::emit(
        "adapter",
        format!(
            "invocation accepted model={} timeout_seconds={}",
            config.params.model, config.params.timeout_seconds
        ),
    );
    let deadline = Instant::now() + Duration::from_secs(config.params.timeout_seconds);
    prepare_context(&mut config, deadline)?;
    let mut provider = runtime::Runtime::spawn(&config, sender)?;
    let mut output = io::stdout().lock();
    let mut session = session::Session::new(&invocation, &config, &mut output);
    session.progress("Starting Claude.")?;
    let mut next_trace = Instant::now() + Duration::from_secs(5);
    loop {
        if runtime::stopping() {
            trace::emit("adapter", "stop observed");
            return Err(error("Claude invocation stopped"));
        }
        if Instant::now() >= deadline {
            trace::emit("adapter", "invocation timeout reached");
            return Err(error("Claude invocation exceeded its timeout"));
        }
        let message = match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(message) => message,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= next_trace {
                    session.trace_state("waiting for input");
                    next_trace = Instant::now() + Duration::from_secs(5);
                }
                continue;
            }
            Err(cause) => return Err(cause.into()),
        };
        if let Some(summary) = input_summary(&message) {
            trace::emit("adapter", summary);
        }
        match message {
            Input::Host(message) => session.host(message)?,
            Input::Mcp(request, stream) => session.mcp(request, stream)?,
            Input::Provider(event) => session.provider(event)?,
            Input::ProviderClosed => session.provider_closed = true,
            Input::HostClosed => return Err(error("CWR host disconnected")),
            Input::Failure(cause) => return Err(error(cause)),
        }
        if session.ready()? {
            trace::emit("adapter", "session ready; cleaning up provider");
            if !session.provider_failed {
                provider.finish()?;
            }
            // Provider cleanup must complete before the host receives a terminal.
            drop(provider);
            trace::emit("adapter", "provider cleanup complete; returning terminal");
            return session.complete();
        }
    }
}

fn input_summary(input: &Input) -> Option<String> {
    match input {
        Input::Host(AdapterHostMessage::Invoke { .. }) => Some("input=host_invoke".into()),
        Input::Host(AdapterHostMessage::CallResult { .. }) => Some("input=host_call_result".into()),
        Input::Host(AdapterHostMessage::ActivityResult { .. }) => {
            Some("input=host_activity_result".into())
        }
        Input::HostClosed => Some("input=host_closed".into()),
        Input::Provider(event)
            if matches!(event["type"].as_str(), Some("assistant" | "stream_event")) =>
        {
            None
        }
        Input::Provider(event) => Some(format!(
            "input=provider type={} subtype={}",
            event["type"].as_str().unwrap_or("unknown"),
            event["subtype"].as_str().unwrap_or("none")
        )),
        Input::ProviderClosed => Some("input=provider_closed".into()),
        Input::Mcp(request, _) => Some(format!(
            "input=mcp method={}",
            request["method"].as_str().unwrap_or("unknown")
        )),
        Input::Failure(_) => Some("input=failure".into()),
    }
}

fn error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message.into()).into()
}

fn prepare_context(config: &mut config::Config, deadline: Instant) -> CwrResult<()> {
    let reserve = config.prompt.len()
        + crate::adapter::context::encoded_bytes(&json!(config.tools))?
        + crate::adapter::context::encoded_bytes(&config.return_schema)?
        + crate::adapter::context::encoded_bytes(&config.observation["action_request"])?
        + 32 * 1024;
    let budget = config.params.context_window_tokens.saturating_sub(reserve);
    if crate::adapter::context::encoded_bytes(&config.observation)? <= budget {
        return Ok(());
    }
    let records = crate::adapter::context::prepare(&config.observation, budget, |records| {
        compact_context(config, records, deadline)
    })?;
    config.observation = json!({"version":"cwr.adapter-working-context/1",
        "context_snapshot":config.observation["context_snapshot"],
        "context_records":records,"action_request":config.observation["action_request"]});
    Ok(())
}

fn compact_context(
    config: &config::Config,
    records: &[Value],
    deadline: Instant,
) -> CwrResult<String> {
    let compact = config::Config {
        working_ref: None,
        working_snapshot: Value::Null,
        provider_store: config.provider_store,
        params: config.params.clone(),
        integration: config.integration.clone(),
        prompt: crate::adapter::context::COMPACTION_INSTRUCTIONS.into(),
        observation: json!({"context_records":records}),
        return_schema: json!({}),
        provider_return_schema: json!({}),
        tools: vec![],
        native_tools: vec![],
        workspace_root: config.workspace_root.clone(),
        tool_namespace: "cwp".into(),
        resume: None,
        native_text: true,
    };
    let (sender, receiver) = mpsc::sync_channel(2);
    let mut provider = runtime::Runtime::spawn(&compact, sender)?;
    loop {
        if runtime::stopping() || Instant::now() >= deadline {
            return Err(error(
                "Claude context preparation stopped before live execution",
            ));
        }
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(Input::Provider(event)) if event["type"] == "result" => {
                if event["subtype"] != "success" || event["is_error"].as_bool() == Some(true) {
                    return Err(error(
                        "Claude context compaction failed before live execution",
                    ));
                }
                let summary = event["result"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                    .ok_or_else(|| error("Claude context compaction returned no summary"))?
                    .to_owned();
                provider.finish()?;
                return Ok(summary);
            }
            Ok(Input::Mcp(request, mut stream)) => {
                if let Some(id) = request.get("id") {
                    let value = match request["method"].as_str() {
                        Some("initialize") => mcp::initialize(&request)?,
                        Some("tools/list") => json!({"tools":[]}),
                        Some("ping") => json!({}),
                        _ => {
                            return Err(error("Claude requested a tool during context preparation"))
                        }
                    };
                    mcp::respond(&mut stream, id, value)?;
                }
            }
            Ok(Input::Provider(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Ok(Input::Failure(cause)) => return Err(error(cause)),
            _ => return Err(error("Claude context preparation ended without a summary")),
        }
    }
}

// The first host message includes its streamed source context. Source size does
// not impose an initialization deadline, but stop still interrupts the wait.
fn receive_initial_context(
    receiver: &mpsc::Receiver<Input>,
    stopped: impl Fn() -> bool,
) -> CwrResult<Input> {
    loop {
        if stopped() {
            return Err(error("Claude stopped while receiving context"));
        }
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(message) => return Ok(message),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(cause) => return Err(cause.into()),
        }
    }
}

#[cfg(test)]
#[path = "claude/initial_context_tests.rs"]
mod initial_context_tests;
