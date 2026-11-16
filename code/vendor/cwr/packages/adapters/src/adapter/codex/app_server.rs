//! One bounded CWR action over the Codex app-server protocol.
use super::open_responses::{CodexOpenResponsesMapper, CompletedAssistantMessage};
use crate::{
    adapter::{AdapterActivityEvent, AdapterCompletedCommentary, AdapterModelOutput},
    CwrResult,
};
use cwr::{
    open_responses::v2026_04_24::AssistantPhase, v2026_09::invocation::ActionResponseStatus,
};
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
pub(crate) const INITIALIZE_ID: &str = "cwr-initialize";
pub(crate) const THREAD_START_ID: &str = "cwr-thread-start";
pub(crate) const OBSERVATION_INJECT_ID: &str = "cwr-observation-inject";
pub(crate) const CONTEXT_COMPACT_ID: &str = "cwr-context-compact";
pub(crate) const TURN_START_ID: &str = "cwr-turn-start";
const PROVIDER_POLL_INTERVAL: Duration = Duration::from_millis(20);
include!("app_server/model.rs");
include!("app_server/service_tier.rs");
include!("app_server/user_input.rs");
include!("app_server/failure.rs");

enum DeferredTurnEnd {
    Returned(BoundedActionResult),
    MissingResult,
}
pub(crate) fn run_bounded_action<W, H, F>(
    provider_input: &mut W,
    provider_lines: &mpsc::Receiver<io::Result<ProviderLine>>,
    config: &mut BoundedActionConfig,
    provider_started: Instant,
    handler: &mut H,
    provider_exited: F,
    provider_inactivity_timeout: Duration,
) -> CwrResult<BoundedActionResult>
where
    W: Write,
    H: BoundedActionHandler,
    F: FnMut() -> CwrResult<bool>,
{
    let mut handler = StartupTimingHandler::new(handler);
    let mut timings = TurnTimings::new(provider_started);
    let result = run_bounded_action_inner(
        provider_input,
        provider_lines,
        config,
        &mut handler,
        provider_exited,
        &mut timings,
        provider_inactivity_timeout,
    );
    timings.stages.insert(
        TimingDuration::ProviderElapsed,
        provider_started.elapsed().as_millis() as u64,
    );
    handler.on_timing(AdapterTiming {
        checkpoint: TimingCheckpoint::ProviderEnded,
        durations_ms: timings.stages,
        context_delivery: None,
        compaction: None,
    });
    result
}
#[expect(
    clippy::too_many_lines,
    reason = "The bounded action loop keeps JSON-RPC sequencing, streaming output, and terminal handling in one state machine."
)]
fn run_bounded_action_inner<W, H, F>(
    provider_input: &mut W,
    provider_lines: &mpsc::Receiver<io::Result<ProviderLine>>,
    config: &mut BoundedActionConfig,
    handler: &mut H,
    mut provider_exited: F,
    timings: &mut TurnTimings,
    provider_inactivity_timeout: Duration,
) -> CwrResult<BoundedActionResult>
where
    W: Write,
    H: BoundedActionHandler,
    F: FnMut() -> CwrResult<bool>,
{
    let mut history = HistoryPreparation::new(config)?;
    history.report_oversized_fork(handler);
    // The provider spool and pinned source digest now own initialization. Keep
    // only the exact incoming request while the execution is alive.
    let action_request = config.observation["action_request"].take();
    config.observation = json!({"action_request":action_request});
    timings.stage(TimingDuration::ContextPreparation);
    write_json_line(
        provider_input,
        &json!({
            "id": INITIALIZE_ID,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": config.client_name,
                    "title": config.client_title,
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {"experimentalApi": true}
            }
        }),
    )?;
    let mut native_thread_id = None;
    let mut service_tier = TurnServiceTier::default();
    let mut open_responses: Option<CodexOpenResponsesMapper> = None;
    let mut awaiting_turn_start_response = false;
    let mut deferred_turn_end = None;
    let mut pending_return = None;
    let mut native_final_answer = None;
    let mut user_inputs = PendingUserInputs::default();
    let mut last_provider_activity = Instant::now();
    loop {
        if handler.is_stopped() {
            return invalid("Codex action execution stopped");
        }
        let waiting_for_user_input = user_inputs.is_waiting();
        user_inputs.poll(provider_input, handler)?;
        if waiting_for_user_input || user_inputs.is_waiting() {
            last_provider_activity = Instant::now();
        }
        let ProviderLine { line, received_at } =
            match provider_lines.recv_timeout(PROVIDER_POLL_INTERVAL) {
                Ok(Ok(line)) => line,
                Ok(Err(error)) => return Err(error.into()),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if provider_exited()? {
                        return invalid("Codex app-server stopped before cwp.cwr--return--result");
                    }
                    if last_provider_activity.elapsed() >= provider_inactivity_timeout {
                        return Err(CodexFailure::unresponsive(provider_inactivity_timeout).into());
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return invalid("Codex app-server output ended before cwp.cwr--return--result")
                }
            };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        last_provider_activity = Instant::now();
        if message.get("id").and_then(Value::as_str) == Some(INITIALIZE_ID) {
            timings.stage(crate::adapter::TimingDuration::ProviderInitialize);
            write_json_line(
                provider_input,
                &json!({"method": "initialized", "params": {}}),
            )?;
            start_thread_with_resume(
                provider_input,
                config,
                history.provider_resume_thread(),
                history.provider_fork_thread(),
            )?;
            continue;
        }
        let context_ready = if message.get("id").and_then(Value::as_str) == Some(THREAD_START_ID) {
            timings.stage(crate::adapter::TimingDuration::ThreadSetup);
            if let Some(error) = message.get("error") {
                return Err(CodexFailure::provider(
                    "Codex could not start the conversation",
                    error,
                )
                .into());
            }
            let thread_id = message
                .pointer("/result/thread/id")
                .and_then(Value::as_str)
                .ok_or("Codex app-server returned no thread ID")?;
            native_thread_id = Some(thread_id.to_owned());
            service_tier.model = message
                .pointer("/result/model")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| config.model.clone());
            history.checkpoint.thread_id = thread_id.to_owned();
            handler.on_activity(AdapterActivityEvent::ProviderThread {
                provider: "codex".into(),
                thread_id: thread_id.into(),
            })?;
            history.advance(provider_input, handler)?
        } else if message.get("id").and_then(Value::as_str) == Some(OBSERVATION_INJECT_ID) {
            if let Some(error) = message.get("error") {
                return invalid(format!(
                    "Codex app-server context injection failed: {error}"
                ));
            }
            history.injected(handler)?;
            history.advance(provider_input, handler)?
        } else if message.get("id").and_then(Value::as_str) == Some(CONTEXT_COMPACT_ID) {
            if let Some(error) = message.get("error") {
                history.fail_compaction(received_at, handler);
                return invalid(format!(
                    "Codex app-server context compaction failed: {error}"
                ));
            }
            if history.compaction_request_accepted(received_at, handler)? {
                history.advance(provider_input, handler)?
            } else {
                false
            }
        } else if !history.checkpoint.live_started {
            if matches!(
                message["method"].as_str(),
                Some("item/tool/call" | "item/tool/requestUserInput")
            ) {
                return invalid(
                    "Codex requested a live tool while context was still being established",
                );
            }
            if history.observe_compaction(&message, received_at, handler)? {
                history.advance(provider_input, handler)?
            } else {
                false
            }
        } else {
            false
        };
        if context_ready || message["id"].as_str() == Some(MODEL_LIST_ID) {
            if !service_tier.prepare(provider_input, config, &message, context_ready)? {
                continue;
            }
            timings.stage(crate::adapter::TimingDuration::ContextInjection);
            let input = history.turn_input(&config.observation["action_request"])?;
            history.begin_live_turn(handler)?;
            timings.turn_sent = Some(Instant::now());
            service_tier.start_turn(provider_input, &native_thread_id, input)?;
            timings.startup_complete(handler);
            awaiting_turn_start_response = true;
            continue;
        }
        if !history.checkpoint.live_started {
            if let Some(activity) = execution_activity(&message, None) {
                handler.on_activity(activity)?;
            }
            if let Some(activity) = provider_activity(&message, None) {
                handler.on_activity(activity)?;
            }
            continue;
        }
        if message.get("id").and_then(Value::as_str) == Some(TURN_START_ID) {
            if let Some(error) = message.get("error") {
                return Err(CodexFailure::provider("Codex could not start the turn", error).into());
            }
            timings.turn_accepted(received_at, handler);
            let thread_id = native_thread_id
                .as_deref()
                .ok_or("Codex app-server returned no thread ID before turn start")?;
            let turn_id = message
                .pointer("/result/turn/id")
                .and_then(Value::as_str)
                .and_then(nonblank)
                .ok_or("Codex app-server returned no turn ID")?;
            establish_open_responses_scope(
                &mut open_responses,
                thread_id,
                turn_id,
                "turn/start response",
            )?;
            awaiting_turn_start_response = false;
            if let Some(turn_end) = deferred_turn_end.take() {
                return match turn_end {
                    DeferredTurnEnd::Returned(result) => Ok(result),
                    DeferredTurnEnd::MissingResult => {
                        invalid("Codex action turn ended without cwp.cwr--return--result")
                    }
                };
            }
            continue;
        }
        if message.get("method").and_then(Value::as_str) == Some("turn/started") {
            let (thread_id, turn_id) = turn_started_scope(&message)?;
            if native_thread_id.as_deref() == Some(thread_id.as_str())
                && (open_responses.is_some() || awaiting_turn_start_response)
            {
                establish_open_responses_scope(
                    &mut open_responses,
                    &thread_id,
                    &turn_id,
                    "turn/started notification",
                )?;
            }
        }
        if user_inputs.observe(&message, open_responses.as_ref(), provider_input, handler)? {
            continue;
        }
        history.observe_live_message(
            &message,
            open_responses
                .as_ref()
                .map(CodexOpenResponsesMapper::turn_id),
        );
        if let Some(scope) = open_responses.as_ref() {
            timings.observe_live_compaction(&message, received_at, scope, handler);
            timings.observe_tool(&message, received_at, scope, handler);
        }
        // Shapes outside the dated subset remain provider diagnostics; they do
        // not fail the action or become commentary.
        if let Some(open_responses) = open_responses.as_mut() {
            if let Some(completed) = process_open_responses_message(
                open_responses,
                &message,
                received_at,
                config.model_run_ref.as_deref(),
                config.native_text_result,
                timings,
                handler,
            )? {
                native_final_answer = Some(completed);
            }
        }
        if let Some(activity) = execution_activity(&message, open_responses.as_ref()) {
            handler.on_activity(activity)?;
        }
        if let Some(activity) = provider_activity(&message, open_responses.as_ref()) {
            handler.on_activity(activity)?;
        }
        if message.get("method").and_then(Value::as_str) == Some("item/tool/call") {
            let request_id = message
                .get("id")
                .cloned()
                .ok_or("Codex dynamic tool call has no request ID")?;
            let namespace = message.pointer("/params/namespace").and_then(Value::as_str);
            let tool = message.pointer("/params/tool").and_then(Value::as_str);
            if pending_return.is_some() {
                write_tool_response(
                    provider_input,
                    request_id,
                    false,
                    &json!({
                        "accepted": false,
                        "error": "Codex action already accepted a terminal result"
                    }),
                )?;
                continue;
            }
            if namespace == Some("cwp")
                && tool == Some("cwr--return--result")
                && !config.native_text_result
            {
                if deferred_turn_end.is_some() || pending_return.is_some() {
                    write_tool_response(
                        provider_input,
                        request_id,
                        false,
                        &json!({
                            "accepted": false,
                            "error": "Codex action turn already ended"
                        }),
                    )?;
                    continue;
                }
                let arguments = message
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(Value::Null);
                if let Err(error) =
                    validate_instance("action return", &config.return_schema, &arguments)
                {
                    write_tool_response(
                        provider_input,
                        request_id,
                        false,
                        &json!({"accepted": false, "error": error.to_string()}),
                    )?;
                    continue;
                }
                let status = arguments["status"]
                    .as_u64()
                    .and_then(|status| u16::try_from(status).ok())
                    .and_then(|status| ActionResponseStatus::new(status).ok())
                    .ok_or_else(|| invalid_error("Codex action return has no valid status"))?;
                if status.is_stopped() {
                    write_tool_response(
                        provider_input,
                        request_id,
                        false,
                        &json!({
                            "accepted": false,
                            "error": "status 499 is reserved for a host-created stop"
                        }),
                    )?;
                    continue;
                }
                if status.is_progress() {
                    let summary = arguments["summary"]
                        .as_str()
                        .ok_or_else(|| invalid_error("Codex action return has no summary"))?;
                    handler.on_public_progress(status.as_u16(), summary)?;
                    write_tool_response(
                        provider_input,
                        request_id,
                        true,
                        &json!({"accepted": true, "terminal": false}),
                    )?;
                    continue;
                }
                if !handler.claim_terminal() {
                    return invalid("Codex action execution stopped");
                }
                write_tool_response(
                    provider_input,
                    request_id,
                    true,
                    &json!({"accepted": true, "terminal": true}),
                )?;
                history.mark_usage_stale();
                pending_return = Some(arguments);
                continue;
            }
            if namespace.is_some() {
                let declared_tool = tool.and_then(|name| {
                    config.host_tools.iter().find(|candidate| {
                        candidate.name == name && Some(candidate.namespace.as_str()) == namespace
                    })
                });
                if let Some(declared_tool) = declared_tool {
                    let call_id = provider_call_id(&message)?;
                    let arguments = message
                        .pointer("/params/arguments")
                        .cloned()
                        .unwrap_or(Value::Null);
                    if let Err(error) = validate_instance(
                        &format!("{} input", declared_tool.operation),
                        &declared_tool.input_schema,
                        &arguments,
                    ) {
                        write_tool_response(
                            provider_input,
                            request_id,
                            false,
                            &json!({"accepted": false, "error": error.to_string()}),
                        )?;
                        continue;
                    }
                    let operation = crate::adapter::AdapterHostOperation {
                        name: declared_tool.operation.clone(),
                        description: declared_tool.description.clone(),
                        input_schema: declared_tool.input_schema.clone(),
                        output_schema: json!({}),
                    };
                    let (operation, arguments) =
                        match crate::adapter::tools::dispatch(&operation, arguments) {
                            Ok(decoded) => decoded,
                            Err(error) => {
                                write_tool_response(
                                    provider_input,
                                    request_id,
                                    false,
                                    &json!({"accepted":false,"error":error.to_string()}),
                                )?;
                                continue;
                            }
                        };
                    let result = handler.on_host_call(call_id, &operation, arguments)?;
                    last_provider_activity = Instant::now();
                    write_host_tool_response(
                        provider_input,
                        request_id,
                        &operation,
                        result.success,
                        &result.content,
                    )?;
                    continue;
                }
            }
            if namespace == Some("cwr") && matches!(tool, Some("send_message" | "invoke_action")) {
                let operation = format!("cwr.{}", tool.unwrap());
                write_tool_response(
                    provider_input,
                    request_id,
                    false,
                    &json!({
                        "accepted": false,
                        "error": format!("{operation} is unavailable")
                    }),
                )?;
                continue;
            }
            write_tool_response(
                provider_input,
                request_id,
                false,
                &json!({
                    "accepted": false,
                    "error": "runtime tool is unavailable in the current action state"
                }),
            )?;
            continue;
        }
        if message.get("method").and_then(Value::as_str) == Some("turn/completed") {
            if !turn_notification_matches_scope(&message, open_responses.as_ref()) {
                continue;
            }
            if let Some(status) = message
                .pointer("/params/turn/status")
                .and_then(Value::as_str)
            {
                if status != "completed" {
                    timings.fail_live_compaction(received_at, handler);
                    let error = &message["params"]["turn"]["error"];
                    return Err(
                        CodexFailure::provider("Codex could not complete the turn", error).into(),
                    );
                }
            }
            if config.native_text_result {
                if !handler.claim_terminal() {
                    return invalid("Codex action execution stopped");
                }
                let returned = native_text_return(config, native_final_answer.take())?;
                if message
                    .pointer("/params/turn/status")
                    .and_then(Value::as_str)
                    != Some("completed")
                {
                    return invalid("Codex private execution did not complete successfully");
                }
                history.retain_working_state(handler)?;
                let result = BoundedActionResult {
                    returned,
                    continuation: serde_json::to_value(&history.checkpoint)?,
                };
                if awaiting_turn_start_response {
                    deferred_turn_end = Some(DeferredTurnEnd::Returned(result));
                    continue;
                }
                return Ok(result);
            }
            if let Some(returned) = pending_return.take() {
                history.retain_working_state(handler)?;
                let result = BoundedActionResult {
                    returned,
                    continuation: serde_json::to_value(&history.checkpoint)?,
                };
                if awaiting_turn_start_response {
                    deferred_turn_end = Some(DeferredTurnEnd::Returned(result));
                    continue;
                }
                return Ok(result);
            }
            if awaiting_turn_start_response {
                deferred_turn_end.get_or_insert(DeferredTurnEnd::MissingResult);
                continue;
            }
            return invalid("Codex action turn ended without cwp.cwr--return--result");
        }
        if message.get("id").is_some() && message.get("method").is_some() {
            write_json_line(
                provider_input,
                &json!({
                    "id": message["id"],
                    "error": {
                        "code": -32601,
                        "message": "CWR does not provide this app-server request"
                    }
                }),
            )?;
        }
    }
}

include!("app_server/output.rs");
include!("app_server/progress.rs");
include!("app_server/wire.rs");
include!("app_server/history.rs");
include!("app_server/timing.rs");
include!("app_server/tests.rs");

include!("app_server/user_input_tests.rs");
