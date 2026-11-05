use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;

const SH_RUN_COMMAND_ACTION: &str = "run--command";
const CWR_INVOCATION_REF_ENV: &str = "CWR_INVOCATION_REF";
const CWR_SOURCE_ACTOR_REF_ENV: &str = "CWR_SOURCE_ACTOR_REF";
const WORKLOOP_SMS_BRIDGE_SOCKET_ENV: &str = "WORKLOOP_SMS_BRIDGE_SOCKET";
const WORKLOOP_SMS_ACTIONS: [&str; 3] = [
    "receive--message",
    "deliver--message",
    "handle--request",
];
const MAX_SHELL_STREAM_BYTES: usize = 64 * 1024;
const SHELL_OUTPUT_TRUNCATED: &[u8] = b"[earlier output truncated]\n";
const SHELL_PROGRESS_PREFIX: &[u8] = b"CWR_PROGRESS\t";
const SHELL_PROGRESS_QUEUE_CAPACITY: usize = 64;
include!("sh/coworker.rs");
include!("sh/coworker_io.rs");
include!("sh/gateway.rs");
include!("sh/output.rs");
include!("sh/runner.rs");
include!("sh/timeout.rs");
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunCommandInput {
    command: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShActorIntegrationParams {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    commands: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    message_commands: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    message_context_json_stdin: Option<bool>,
    #[serde(default)]
    fail_on_nonzero: bool,
    #[serde(default)]
    stream_output: bool,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    host_env_actions: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    progress_jsonl_stderr_actions: std::collections::BTreeSet<String>,
    #[serde(default)]
    activated_volumes: std::collections::BTreeMap<String, ActivatedShellVolume>,
}
#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ActivatedShellVolume {
    path: String,
    access: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellProgress {
    summary: String,
    #[serde(default)]
    published: bool,
    #[serde(default, deserialize_with = "shell_present_progress")]
    progress: Option<cwr::v2026_09::invocation::ActionProgress>,
}
fn shell_present_progress<'de, D: serde::Deserializer<'de>>(deserializer: D)
    -> Result<Option<cwr::v2026_09::invocation::ActionProgress>, D::Error> {
    Deserialize::deserialize(deserializer).map(Some)
}
#[derive(Debug)]
struct CommandResult {
    command: String,
    exit_code: i32,
    stdout: String,
    stderr: String,
    timed_out_after_ms: Option<u64>,
}
#[derive(Clone, Copy)]
struct ShellCommandOptions {
    stream_output: bool,
    progress_jsonl: bool,
    timeout: Duration,
}
pub fn run_sh_adapter(
    input: impl BufRead + Send + 'static,
    output: impl Write,
) -> CwrResult<()> {
    run_sh_adapter_with_progress_fd(input, output, None)
}

pub fn run_sh_adapter_process(
    input: impl BufRead + Send + 'static,
    output: impl Write + AsRawFd,
) -> CwrResult<()> {
    let progress_fd = output.as_raw_fd();
    run_sh_adapter_with_progress_fd(input, output, Some(progress_fd))
}

fn run_sh_adapter_with_progress_fd(
    mut input: impl BufRead + Send + 'static,
    mut output: impl Write,
    progress_fd: Option<RawFd>,
) -> CwrResult<()> {
    let message: AdapterHostMessage = read_adapter_message(&mut input)?
        .ok_or_else(|| protocol_error("Shell adapter received no invocation"))?;
    let (protocol, invocation) = match message {
        AdapterHostMessage::Invoke {
            protocol,
            invocation,
        } => (protocol, invocation),
        AdapterHostMessage::CallResult { .. } => {
            return protocol_failure("Shell adapter received a call result before invocation")
        }
        AdapterHostMessage::ActivityResult { .. } => {
            return protocol_failure("Shell adapter received an activity result before invocation")
        }
    };
    if protocol != PROCESS_ADAPTER_PROTOCOL_REF {
        return protocol_failure(format!(
            "Shell adapter supports {PROCESS_ADAPTER_PROTOCOL_REF}, not {protocol}"
        ));
    }
    validate_adapter_invocation(&invocation)?;
    let params: ShActorIntegrationParams =
        serde_json::from_value(invocation.actor_integration_params.clone()).map_err(|error| {
            protocol_error(format!(
                "Shell actor integration params are invalid: {error}"
            ))
        })?;
    let runner = ShellCommandRunner::from_params(invocation.integration_params.clone())?
        .with_action_host_environment(&params, &invocation.action_name)?;
    write_adapter_message(
        &mut output,
        &AdapterEvent::Progress {
                progress: None,
                published: false,
            invocation_ref: invocation.invocation_ref.clone(),
            sequence: 1,
            status: 102,
            summary: "Running shell command.".into(),
        },
    )?;
    let invocation_ref = invocation.invocation_ref.clone();
    let source_actor_ref = invocation.source_actor_ref.clone();
    let invoke_operation = invocation
        .host_operations
        .iter()
        .find(|operation| matches!(operation.name.as_str(), crate::action_contract::INVOKE_ACTION_HOST_OPERATION | "cwr.send_request"))
        .cloned();
    let (host_input_sender, host_input) = std::sync::mpsc::channel();
    thread::spawn(move || loop {
        let message = read_adapter_message(&mut input).map_err(|error| error.to_string());
        let finished = !matches!(message, Ok(Some(_)));
        if host_input_sender.send(message).is_err() || finished {
            break;
        }
    });
    let mut events = ShellAdapterEvents {
        input: ShellHostInput::Async(host_input, std::marker::PhantomData),
        output: &mut output,
        invocation_ref: &invocation_ref,
        source_actor_ref: source_actor_ref.as_deref(),
        next_sequence: 2,
        progress_fd,
        gateway_operation: invocation.host_operations.iter().find(|op| op.name == "cwr.send_request").cloned(),
        coworker_actions_available: ["cwr.list_actions", crate::action_contract::INVOKE_ACTION_HOST_OPERATION]
            .iter().all(|name| invocation.host_operations.iter().any(|op| op.name == *name)),
    };
    let message_context_json_stdin = params.message_context_json_stdin.unwrap_or_default();
    let command = if invocation.action_name == RESPOND_TO_MESSAGE_ACTION
        && (!params.message_commands.is_empty() || message_context_json_stdin)
    {
        None
    } else {
        params
            .commands
            .get(&invocation.action_name)
            .cloned()
            .or_else(|| params.command.clone())
    };
    let stream_output = params.stream_output;
    let result = match (command, invocation.action_name.as_str()) {
        (Some(command), _) => {
            let progress_jsonl = params
                .progress_jsonl_stderr_actions
                .contains(&invocation.action_name);
            command_result_json(run_command(
                command,
                &runner,
                Some(&invocation.input),
                ShellCommandOptions {
                    stream_output,
                    progress_jsonl,
                    timeout: shell_command_timeout(params.timeout_ms)?,
                },
                invoke_operation.as_ref(),
                &mut events,
            )?)
        }
        (None, SH_RUN_COMMAND_ACTION) => {
            let requested: RunCommandInput = serde_json::from_value(invocation.input.clone())
                .map_err(|error| {
                    protocol_error(format!("Shell command input is invalid: {error}"))
                })?;
            command_result_json(run_command_without_progress(
                requested.command,
                &runner,
                stream_output,
                None,
                shell_command_timeout(requested.timeout_ms.or(params.timeout_ms))?,
                invoke_operation.as_ref(),
                &mut events,
            )?)
        }
        (None, RESPOND_TO_MESSAGE_ACTION) => respond_to_shell_message(
            &invocation,
            &params,
            &runner,
            invoke_operation.as_ref(),
            &mut events,
        )?,
        (None, action) => {
            let sequence = events.next_sequence;
            return write_adapter_message(
                &mut output,
                &AdapterEvent::Result {
                    invocation_ref,
                    sequence,
                    status: 422,
                    summary: format!("Shell does not support action {action}."),
                    result: None,
                    error: Some(AdapterError {
                        code: "unsupported_action".into(),
                        message: Some(format!("Shell does not support action {action}")),
                        details: None,
                    }),
                    continuation: invocation.continuation,
                },
            );
        }
    };
    let sequence = events.next_sequence;
    let timed_out = result
        .get("timed_out_after_ms")
        .and_then(Value::as_u64);
    let failed_exit = result
        .get("exit_code")
        .and_then(Value::as_i64)
        .filter(|code| params.fail_on_nonzero && *code != 0);
    let terminal = AdapterEvent::Result {
            invocation_ref: invocation_ref.clone(),
            sequence,
            status: if timed_out.is_some() { 504 } else if failed_exit.is_some() { 500 } else { 200 },
            summary: timed_out.map_or_else(
                || failed_exit.map_or_else(
                    || "Ran shell command.".into(),
                    |code| format!("Shell command failed with exit code {code}."),
                ),
                |timeout_ms| format!("Shell command timed out after {timeout_ms} ms."),
            ),
            result: (timed_out.is_none() && failed_exit.is_none()).then(|| result.clone()),
            error: timed_out.map(|timeout_ms| AdapterError {
                code: "command_timeout".into(),
                message: Some(format!("Shell command timed out after {timeout_ms} ms")),
                details: Some(result.clone()),
            }).or_else(|| failed_exit.map(|code| AdapterError {
                code: "command_failed".into(),
                message: Some(format!("Shell command exited with code {code}")),
                details: Some(result),
            })),
            continuation: invocation.continuation,
        };
    events.terminal(&terminal)
}

#[cfg(test)]
include!("sh/tests.rs");
fn respond_to_shell_message(
    invocation: &AdapterInvocation,
    params: &ShActorIntegrationParams,
    runner: &ShellCommandRunner,
    invoke_operation: Option<&AdapterHostOperation>,
    events: &mut ShellAdapterEvents<'_, impl Write>,
) -> CwrResult<Value> {
    let message_commands = &params.message_commands;
    let source_message_id = invocation
        .input
        .get("source_message_ref")
        .or_else(|| invocation.input.get("source_message_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| protocol_error("Shell message invocation has no source message id"))?;
    let messages = invocation
        .thread
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol_error("Shell invocation thread has no messages"))?;
    let source_index = messages
        .iter()
        .position(|message| {
            message
                .get("message_ref")
                .or_else(|| message.get("message_id"))
                .and_then(Value::as_str)
                == Some(source_message_id)
        })
        .ok_or_else(|| protocol_error("Shell source message is missing from the thread"))?;
    let source_message = &messages[source_index];
    let source_text = source_message
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| protocol_error("Shell source message is missing from the thread"))?;
    let command = if !message_commands.is_empty() {
        if let Some(command) = message_commands.get(message_command_name(source_text)) {
            command.clone()
        } else {
            let available = message_commands
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(json!({"text": format!("Available commands: {available}.")}));
        }
    } else if let Some(command) = params.command.as_deref() {
        command.to_owned()
    } else {
        shell_command_from_message(source_text).to_owned()
    };
    if command.is_empty() {
        return Ok(json!({"text": "Tell me what command to run."}));
    }
    let message_context = params.message_context_json_stdin.unwrap_or_default().then(|| {
        json!({
            "source_message": source_message,
            "previous_message": source_index.checked_sub(1).map(|index| &messages[index]),
        })
    });
    let result = run_command_without_progress(
        command,
        runner,
        params.stream_output,
        message_context.as_ref(),
        shell_command_timeout(params.timeout_ms)?,
        invoke_operation,
        events,
    )?;
    let text = command_result_text(&result);
    if result.timed_out_after_ms.is_some() {
        let mut value = command_result_json(result);
        value["text"] = text.into();
        Ok(value)
    } else {
        Ok(json!({"text": text}))
    }
}
fn message_command_name(message: &str) -> &str {
    let trimmed = message.trim();
    let Some((leading, rest)) = trimmed.split_once(char::is_whitespace) else {
        return if trimmed.starts_with('@') {
            ""
        } else {
            trimmed
        };
    };
    if leading.starts_with('@') {
        rest.trim()
    } else {
        trimmed
    }
}
fn shell_command_from_message(message: &str) -> &str {
    let trimmed = message.trim();
    let Some(rest) = trimmed.strip_prefix("@sh") else {
        return trimmed;
    };
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        rest.trim_start()
    } else {
        trimmed
    }
}
fn run_command_without_progress(
    command: String,
    runner: &ShellCommandRunner,
    stream_output: bool,
    input_json: Option<&Value>,
    timeout: Duration,
    invoke_operation: Option<&AdapterHostOperation>,
    events: &mut ShellAdapterEvents<'_, impl Write>,
) -> CwrResult<CommandResult> {
    run_command(
        command,
        runner,
        input_json,
        ShellCommandOptions {
            stream_output,
            progress_jsonl: false,
            timeout,
        },
        invoke_operation,
        events,
    )
}
fn run_command(
    command: String,
    runner: &ShellCommandRunner,
    input_json: Option<&Value>,
    options: ShellCommandOptions,
    invoke_operation: Option<&AdapterHostOperation>,
    events: &mut ShellAdapterEvents<'_, impl Write>,
) -> CwrResult<CommandResult> {
    let ShellCommandOptions {
        stream_output,
        progress_jsonl,
        timeout,
    } = options;
    let mut coworker_bridge = invoke_operation
        .map(ShellCoworkerBridge::bind)
        .transpose()?;
    let mut child_command = runner.command(&command);
    child_command
        .process_group(0)
        .env(CWR_INVOCATION_REF_ENV, events.invocation_ref)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(source_actor_ref) = &events.source_actor_ref {
        child_command.env(CWR_SOURCE_ACTOR_REF_ENV, source_actor_ref);
    } else {
        child_command.env_remove(CWR_SOURCE_ACTOR_REF_ENV);
    }
    if let Some(bridge) = &coworker_bridge {
        child_command.env(COWORKER_INVOKE_SOCKET_ENV, bridge.socket());
    } else {
        child_command.env_remove(COWORKER_INVOKE_SOCKET_ENV);
    }
    if input_json.is_some() {
        child_command.stdin(Stdio::piped());
    } else {
        // The adapter's stdin carries process-protocol replies and stays open
        // while the command runs. Never let an ordinary shell command inherit
        // it: stdin-aware tools would consume protocol bytes or wait forever.
        child_command.stdin(Stdio::null());
    }
    if shell_is_stopping() {
        return protocol_failure("Shell command stopped");
    }
    let mut child = child_command
        .spawn()
        .map_err(|error| protocol_error(format!("could not run shell command: {error}")))?;
    let command_pid = child.id();
    before_shell_lifecycle_registration();
    let _lifecycle = match ShellCommandLifecycle::begin() {
        Ok(lifecycle) => lifecycle,
        Err(error) => {
            let terminate_error = terminate_registered_shell_process_group(command_pid).err();
            let reap_error = child.wait().err().map(|cleanup_error| {
                protocol_error(format!(
                    "could not reap stopped shell command after {error}: {cleanup_error}"
                ))
            });
            return Err(terminate_error.or(reap_error).unwrap_or(error));
        }
    };
    let mut deadline = ShellCommandDeadline::start(command_pid, timeout);
    if shell_is_stopping() {
        let deadline_error = deadline.cancel_and_join().err();
        let terminate_error = terminate_shell_process_group(command_pid).err();
        let reap_error = child.wait().err().map(|error| {
            protocol_error(format!("could not reap stopped shell command: {error}"))
        });
        if let Some(error) = deadline_error.or(terminate_error).or(reap_error) {
            return Err(error);
        }
        return protocol_failure("Shell command stopped");
    }
    if stream_output {
        eprintln!(
            "cwr-adapter-sh: command started adapter_pid={} command_pid={command_pid}",
            std::process::id()
        );
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| protocol_error("shell command stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| protocol_error("shell command stderr is unavailable"))?;
    let (progress_sender, progress_receiver) =
        std::sync::mpsc::sync_channel(SHELL_PROGRESS_QUEUE_CAPACITY);
    let stdout_capture = std::sync::Arc::new(ShellOutputCapture::new());
    let stdout_capture_reader = stdout_capture.clone();
    let stdout_reader = thread::spawn(move || {
        let result = read_capped_shell_output_into(stdout, stream_output, &stdout_capture_reader);
        stdout_capture_reader.finished.store(true, Ordering::Release);
        result
    });
    let stderr_capture = std::sync::Arc::new(ShellOutputCapture::new());
    let stderr_capture_reader = stderr_capture.clone();
    let stderr_reader = thread::spawn(move || {
        let result = read_capped_shell_output_with_progress_into(
            stderr,
            stream_output,
            progress_jsonl.then_some(progress_sender),
            &stderr_capture_reader,
        );
        stderr_capture_reader.finished.store(true, Ordering::Release);
        result
    });
    // Keep output draining and coworker calls responsive while the command
    // consumes its input. Either pipe may fill before it starts reading stdin.
    let stdin_writer = input_json.map(|input_json| {
        let stdin = child.stdin.take().expect("piped shell stdin");
        let input_json = input_json.clone();
        thread::spawn(move || write_shell_action_input(stdin, &input_json))
    });
    let wait = match wait_for_shell_command(
        &mut child,
        (&stdout_capture, &stderr_capture),
        &mut deadline,
        stream_output,
        &progress_receiver,
        coworker_bridge.as_mut(),
        events,
    ) {
        Ok(wait) => wait,
        Err(error) => {
            let deadline_error = deadline.cancel_and_join().err();
            let terminate_error = terminate_shell_process_group(command_pid).err();
            let reap_error = child.wait().err().map(|cleanup_error| {
                protocol_error(format!(
                    "could not reap shell command after {error}: {cleanup_error}"
                ))
            });
            return Err(deadline_error
                .or(terminate_error)
                .or(reap_error)
                .unwrap_or(error));
        }
    };
    drop(deadline);
    if stream_output && wait.timed_out_after_ms.is_none() {
        eprintln!(
            "\ncwr-adapter-sh: command exited command_pid={command_pid} status={}; output pipes closed",
            wait.status
        );
    }
    if wait.timed_out_after_ms.is_none() {
        if let Some(writer) = stdin_writer.filter(|writer| writer.is_finished()) {
            writer
                .join()
                .map_err(|_| protocol_error("shell command stdin writer panicked"))??;
        }
        stdout_reader
            .join()
            .map_err(|_| protocol_error("shell command stdout reader panicked"))??;
        stderr_reader
            .join()
            .map_err(|_| protocol_error("shell command stderr reader panicked"))??;
    }
    let stdout = stdout_capture.snapshot();
    let stderr = stderr_capture.snapshot();
    drain_shell_progress(&progress_receiver, events)?;
    if stream_output {
        eprintln!("cwr-adapter-sh: output pipes closed command_pid={command_pid}");
    }
    Ok(CommandResult {
        command,
        exit_code: wait.status.code().unwrap_or(-1),
        stdout,
        stderr,
        timed_out_after_ms: wait.timed_out_after_ms,
    })
}
