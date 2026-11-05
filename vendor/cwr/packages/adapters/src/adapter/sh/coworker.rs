use std::{
    os::unix::net::{UnixListener as CoworkerListener, UnixStream as CoworkerStream},
    path::{Path as CoworkerPath, PathBuf as CoworkerPathBuf},
};

const COWORKER_INVOKE_SOCKET_ENV: &str = "CWR_COWORKER_INVOKE_SOCKET";
const SHELL_ACTION_CALL_ID: &str = "shell-coworker-invoke-1";
const MAX_COWORKER_MESSAGE_BYTES: usize = cwr::adapter::MAX_PROTOCOL_LINE_BYTES;
const HOST_FRAME_COMPLETION_TIMEOUT: Duration = Duration::from_millis(100);

#[cfg(test)]
thread_local! {
    static SHELL_HOST_FRAME_AFTER_FIRST_WRITE_HOOK: std::cell::Cell<Option<fn()>> = const { std::cell::Cell::new(None) };
}

fn shell_host_frame_after_first_write() {
    #[cfg(test)]
    SHELL_HOST_FRAME_AFTER_FIRST_WRITE_HOOK
        .with(|hook| hook.take().into_iter().for_each(|hook| hook()));
}

type ShellHostMessage = Result<Option<AdapterHostMessage>, String>;

enum ShellHostInput<'a> {
    #[cfg(test)]
    Direct(&'a mut dyn BufRead),
    Async(
        std::sync::mpsc::Receiver<ShellHostMessage>,
        std::marker::PhantomData<&'a ()>,
    ),
}

struct ShellAdapterEvents<'a, W: Write> {
    input: ShellHostInput<'a>,
    output: &'a mut W,
    invocation_ref: &'a str,
    source_actor_ref: Option<&'a str>,
    next_sequence: u64,
    progress_fd: Option<RawFd>,
    gateway_operation: Option<AdapterHostOperation>,
    coworker_actions_available: bool,
}

impl<W: Write> ShellAdapterEvents<'_, W> {
    fn progress(&mut self, record: ShellProgress) -> CwrResult<()> {
        let event = AdapterEvent::Progress {
                progress: record.progress,
                published: record.published,
                invocation_ref: self.invocation_ref.into(),
                sequence: self.next_sequence,
                status: 102,
                summary: record.summary,
            };
        if let Some(fd) = self.progress_fd {
            if !write_progress_frame_nonblocking(fd, &event)? {
                return Ok(());
            }
        } else {
            write_adapter_message(self.output, &event)?;
        }
        self.next_sequence += 1;
        Ok(())
    }

    fn receive_until(&mut self, deadline: Instant) -> CwrResult<Option<AdapterHostMessage>> {
        match &mut self.input {
            #[cfg(test)]
            ShellHostInput::Direct(input) => read_adapter_message(input),
            ShellHostInput::Async(input, _) => loop {
                if shell_is_stopping() || Instant::now() >= deadline {
                    return Ok(None);
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                match input.recv_timeout(remaining.min(Duration::from_millis(100))) {
                    Ok(Ok(message)) => return Ok(message),
                    Ok(Err(error)) => return protocol_failure(error),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        return protocol_failure("Shell host input reader stopped")
                    }
                }
            },
        }
    }

    fn discover_actions(&mut self, deadline: Instant) -> CwrResult<Option<Value>> {
        const CALL_ID: &str = "shell-coworker-list-1";
        if !self.write_host_event_until(&AdapterEvent::Call {
            invocation_ref: self.invocation_ref.into(), call_id: CALL_ID.into(),
            operation: "cwr.list_actions".into(), input: json!({}),
        }, deadline)? {
            return Ok(None);
        }
        match self.receive_until(deadline)? {
            Some(AdapterHostMessage::CallResult { invocation_ref, call_id, result, error })
                if invocation_ref == self.invocation_ref && call_id == CALL_ID => {
                    match (result, error) {
                        (Some(result), None) => Ok(Some(result)),
                        (None, Some(_error)) => Err(protocol_error(
                            "CWR action discovery failed (host_call_failed)")),
                        _ => protocol_failure("CWR returned invalid action discovery"),
                    }
                }
            None => Ok(None),
            _ => protocol_failure("CWR returned action discovery for another call"),
        }
    }

    fn invoke_action(
        &mut self,
        target: Value,
        input: Value,
        deadline: Instant,
    ) -> CwrResult<Option<Value>> {
        let operation = crate::action_contract::INVOKE_ACTION_HOST_OPERATION;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        let timeout_ms = remaining.as_millis().saturating_add(1).min(u128::from(u64::MAX)) as u64;
        let input = json!({"target": target, "input": input, "timeout_ms": timeout_ms});
        let event = AdapterEvent::Call {
            invocation_ref: self.invocation_ref.into(),
            call_id: SHELL_ACTION_CALL_ID.into(),
            operation: operation.into(),
            input,
        };
        if !self.write_host_event_until(&event, deadline)? {
            return Ok(None);
        }
        let Some(response) = self.receive_until(deadline)? else {
            return Ok(None);
        };
        match response {
            AdapterHostMessage::CallResult {
                invocation_ref,
                call_id,
                result,
                error,
            } if invocation_ref == self.invocation_ref && call_id == SHELL_ACTION_CALL_ID => {
                match (result, error) {
                    (Some(result), None) => {
                        if result.get("state").and_then(Value::as_str) == Some("admitted") {
                            let result: crate::action_contract::StartActionResult =
                                serde_json::from_value(result).map_err(|error| {
                                    protocol_error(format!(
                                        "CWR returned an invalid child admission: {error}"
                                    ))
                                })?;
                            Ok(Some(json!({"type":"started", "result":result})))
                        } else {
                            let result: cwr::v2026_09::invocation::ActionResponse =
                                serde_json::from_value(result).map_err(|error| {
                                    protocol_error(format!(
                                        "CWR returned an invalid coworker result: {error}"
                                    ))
                                })?;
                            Ok(Some(json!({"type":"completed", "result":result})))
                        }
                    }
                    (None, Some(error)) => Ok(Some(json!({
                        "type":"error",
                        "message":error.message.unwrap_or(error.code)
                    }))),
                    _ => protocol_failure("CWR returned an invalid coworker result"),
                }
            }
            AdapterHostMessage::CallResult { .. } => {
                protocol_failure("CWR returned a coworker result for another call")
            }
            AdapterHostMessage::Invoke { .. } => {
                protocol_failure("CWR invoked the Shell adapter while it awaited a coworker")
            }
            AdapterHostMessage::ActivityResult { .. } => protocol_failure(
                "CWR returned an activity result while the Shell adapter awaited a coworker",
            ),
        }
    }

    fn write_host_event_until(
        &mut self,
        event: &AdapterEvent,
        deadline: Instant,
    ) -> CwrResult<bool> {
        let Some(fd) = self.progress_fd else {
            write_adapter_message(self.output, event)?;
            return Ok(true);
        };
        let mut encoded = Vec::new();
        write_adapter_message(&mut encoded, event)?;
        write_frame_nonblocking_until(fd, &encoded, deadline)
    }

    fn terminal(&mut self, event: &AdapterEvent) -> CwrResult<()> {
        let Some(fd) = self.progress_fd else {
            return write_adapter_message(self.output, event);
        };
        let mut encoded = Vec::new();
        write_adapter_message(&mut encoded, event)?;
        write_terminal_frame_nonblocking_until(
            fd,
            &encoded,
            Instant::now() + Duration::from_millis(100),
        )
    }
}

fn write_frame_nonblocking_until(fd: RawFd, encoded: &[u8], deadline: Instant) -> CwrResult<bool> {
    // Before the first byte is accepted, the call remains cancellable and may
    // cleanly give way to the command's terminal event. Once a frame starts,
    // give it a short completion window even if the command deadline expires.
    // Abandoning a partial JSONL frame means the adapter must fail and close
    // stdout; appending a terminal event would corrupt both frames.
    // SAFETY: fd is the live adapter-output descriptor supplied by the caller.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: fd is live and the flags preserve all existing descriptor state.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error().into());
    }
    let result = (|| {
        let mut written = 0;
        let mut completion_deadline = None;
        while written < encoded.len() {
            if written == 0 {
                if shell_is_stopping() || Instant::now() >= deadline {
                    return Ok(false);
                }
            } else if shell_is_stopping() {
                return protocol_failure("stopped while completing a Shell host call frame");
            } else if completion_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return protocol_failure("timed out completing a Shell host call frame");
            }
            // SAFETY: the unwritten suffix is valid for its full length and fd is live.
            let count = unsafe {
                libc::write(
                    fd,
                    encoded[written..].as_ptr().cast(),
                    encoded.len() - written,
                )
            };
            if count > 0 {
                if written == 0 {
                    completion_deadline = Some(
                        deadline.max(Instant::now() + HOST_FRAME_COMPLETION_TIMEOUT),
                    );
                    shell_host_frame_after_first_write();
                }
                written += count as usize;
            } else if count == -1
                && io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock
            {
                let delay = if written == 0 {
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(10))
                } else {
                    completion_deadline
                        .expect("partial frame has a completion deadline")
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(10))
                };
                thread::sleep(delay);
            } else if count == -1
                && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
            {
                continue;
            } else if count == 0 {
                return Err(io::Error::from(io::ErrorKind::WriteZero).into());
            } else {
                return Err(io::Error::last_os_error().into());
            }
        }
        Ok(true)
    })();
    // SAFETY: fd is live and flags came from F_GETFL for this descriptor.
    let restore = unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
    if restore == -1 {
        return Err(io::Error::last_os_error().into());
    }
    result
}

fn write_progress_frame_nonblocking(fd: RawFd, event: &AdapterEvent) -> CwrResult<bool> {
    let mut encoded = serde_json::to_vec(event)?;
    encoded.push(b'\n');
    // POSIX guarantees an all-or-nothing nonblocking write only through the
    // descriptor's PIPE_BUF. Larger best-effort progress frames must be
    // dropped before writing so they cannot leave a partial JSONL frame.
    // SAFETY: fd is the live adapter-output descriptor supplied by the caller.
    let atomic_pipe_frame_bytes = unsafe { libc::fpathconf(fd, libc::_PC_PIPE_BUF) };
    if atomic_pipe_frame_bytes <= 0 || encoded.len() > atomic_pipe_frame_bytes as usize {
        return Ok(false);
    }
    // SAFETY: fd is the live adapter stdout descriptor. The adapter owns all
    // writes, changes the flag only around one atomic-sized frame, and restores
    // the original flags before returning.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: fd is live and the flags preserve all existing descriptor state.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: encoded is valid for its full length and fd is live.
    let written = unsafe { libc::write(fd, encoded.as_ptr().cast(), encoded.len()) };
    let write_error = (written == -1).then(io::Error::last_os_error);
    // SAFETY: fd is live and flags came from F_GETFL for this descriptor.
    let restore = unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
    if restore == -1 {
        return Err(io::Error::last_os_error().into());
    }
    if let Some(error) = write_error {
        return match error.kind() {
            io::ErrorKind::WouldBlock => Ok(false),
            _ => Err(error.into()),
        };
    }
    if written as usize != encoded.len() {
        return protocol_failure("Shell progress frame was only partially written");
    }
    Ok(true)
}

fn write_terminal_frame_nonblocking_until(
    fd: RawFd,
    encoded: &[u8],
    delivery_deadline: Instant,
) -> CwrResult<()> {
    // A terminal is the final frame. It may exceed PIPE_BUF, so a timed-out
    // partial delivery is allowed to end at EOF, where the process adapter
    // reports the incomplete frame. No later JSONL can be corrupted by it.
    // SAFETY: fd is the live adapter-output descriptor supplied by the caller.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: fd is live and the flags preserve all existing descriptor state.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error().into());
    }
    let result = (|| {
        let mut written = 0;
        while written < encoded.len() {
            if Instant::now() >= delivery_deadline {
                return protocol_failure(
                    "timed out delivering the Shell command terminal to the host",
                );
            }
            // SAFETY: encoded is valid for its full length and fd is live.
            let count = unsafe {
                libc::write(
                    fd,
                    encoded[written..].as_ptr().cast(),
                    encoded.len() - written,
                )
            };
            if count > 0 {
                written += count as usize;
            } else if count == -1
                && matches!(
                    io::Error::last_os_error().kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                )
            {
                thread::sleep(
                    delivery_deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(5)),
                );
            } else if count == 0 {
                return Err(io::Error::from(io::ErrorKind::WriteZero).into());
            } else {
                return Err(io::Error::last_os_error().into());
            }
        }
        Ok(())
    })();
    // SAFETY: fd is live and flags came from F_GETFL for this descriptor.
    let restore = unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
    if restore == -1 {
        return Err(io::Error::last_os_error().into());
    }
    result
}

struct ShellCoworkerBridge {
    _directory: tempfile::TempDir,
    socket: CoworkerPathBuf,
    listener: CoworkerListener,
    handled: bool,
}

impl ShellCoworkerBridge {
    fn bind(_invoke_operation: &AdapterHostOperation) -> CwrResult<Self> {
        let directory = tempfile::Builder::new()
            .prefix("cwr-sh-")
            .tempdir_in("/tmp")?;
        let socket = directory.path().join("coworker.sock");
        let listener = CoworkerListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            _directory: directory,
            socket,
            listener,
            handled: false,
        })
    }

    fn socket(&self) -> &CoworkerPath {
        &self.socket
    }

    fn poll<W: Write>(
        &mut self,
        events: &mut ShellAdapterEvents<'_, W>,
        deadline: Instant,
    ) -> CwrResult<bool> {
        let (mut stream, _) = match self.listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(true),
            Err(error) => return Err(error.into()),
        };
        stream.set_nonblocking(false)?;
        if self.handled {
            return write_coworker_message(
                &mut stream,
                &json!({"type":"error", "message":"this Shell command already started a coworker"}),
                deadline,
            );
        }
        self.handled = true;
        self.handle(&mut stream, events, deadline)
    }

    fn handle<W: Write>(
        &self,
        stream: &mut CoworkerStream,
        events: &mut ShellAdapterEvents<'_, W>,
        deadline: Instant,
    ) -> CwrResult<bool> {
        let mut received = Vec::new();
        let Some(describe) = read_coworker_message(stream, &mut received, deadline)? else {
            return Ok(false);
        };
        if matches!(describe["type"].as_str(), Some("describe_request" | "send_request")) {
            return handle_shell_gateway(stream, events, deadline, describe, received);
        }
        if !events.coworker_actions_available {
            return write_coworker_message(
                stream,
                &json!({"type":"error", "message":"coworker actions are unavailable"}),
                deadline,
            );
        }
        let activation = request_activation(&describe, "describe_action")?;
        let Some(catalog) = events.discover_actions(deadline)? else {
            return Ok(false);
        };
        let actions = catalog["actions"].as_array().ok_or("invalid action discovery response")?;
        let matching_actions = actions.iter()
            .filter(|action| action["activation"].as_str() == Some(activation)).count();
        let matches = actions.iter().filter(|action| action["activation"].as_str() == Some(activation)
                && action["is_default_binding"].as_bool().unwrap_or(true))
            .collect::<Vec<_>>();
        let [action] = matches.as_slice() else {
            if !write_coworker_message(stream, &json!({"type":"error",
                "message":format!("action {activation} is unavailable or ambiguous"),
                "diagnostic": {
                    "stage": "discovery",
                    "reason": if matches.is_empty() { "action_unavailable" } else { "action_ambiguous" },
                    "catalog_actions": actions.len(),
                    "matching_actions": matching_actions,
                    "default_matching_actions": matches.len()
                }}), deadline)? {
                return Ok(false);
            }
            return Ok(true);
        };
        let input_schema = &action["input_schema"];
        let target = action.get("target").filter(|target| target.is_object())
            .ok_or("action discovery has no exact target")?;
        if !write_coworker_message(
            stream,
            &json!({
                "type":"action",
                "activation":activation,
                "input_schema":input_schema
            }),
            deadline,
        )? {
            return Ok(false);
        }

        let Some(invoke) = read_coworker_message(stream, &mut received, deadline)? else {
            return Ok(false);
        };
        let invoked_activation = request_activation(&invoke, "invoke_action")?;
        if invoked_activation != activation {
            return write_coworker_message(
                stream,
                &json!({"type":"error", "message":"coworker command changed actions after validation"}),
                deadline,
            );
        }
        let input = invoke
            .get("input")
            .cloned()
            .ok_or_else(|| protocol_error("coworker command sent no action input"))?;
        if !jsonschema::is_valid(input_schema, &input) {
            return write_coworker_message(stream, &json!({"type":"error", "message":"action input does not match its discovered schema"}), deadline);
        }
        let Some(response) = events.invoke_action(target.clone(), input, deadline)? else {
            return Ok(false);
        };
        write_coworker_message(stream, &response, deadline)
    }
}

fn request_activation<'a>(request: &'a Value, expected_type: &str) -> CwrResult<&'a str> {
    if request.get("type").and_then(Value::as_str) != Some(expected_type) {
        return protocol_failure(format!("coworker command expected {expected_type}"));
    }
    request
        .get("activation")
        .and_then(Value::as_str)
        .filter(|activation| !activation.trim().is_empty())
        .ok_or_else(|| protocol_error("coworker command sent no action activation"))
}
