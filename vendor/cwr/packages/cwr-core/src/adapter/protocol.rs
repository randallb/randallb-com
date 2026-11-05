include!("protocol/mailbox.rs");

struct ProtocolReader {
    mailbox: Arc<ProtocolMailbox>,
    cancellation: UnixStream,
    thread: thread::JoinHandle<()>,
}

fn spawn_protocol_reader(
    stdout: impl Read + AsRawFd + Send + 'static,
    expected_invocation_ref: String,
    expected_model_run_ref: Option<String>,
    model_output: AdapterModelOutputMailbox,
    timing: AdapterTimingSink,
) -> CwrResult<ProtocolReader> {
    let mailbox = Arc::new(ProtocolMailbox::new());
    let reader_mailbox = Arc::clone(&mailbox);
    let (cancellation, reader_cancellation) = UnixStream::pair()?;
    let thread = thread::spawn(move || {
        protocol_reader_loop(
            stdout,
            reader_cancellation,
            reader_mailbox.clone(),
            expected_invocation_ref,
            expected_model_run_ref,
            model_output,
            timing,
        );
        reader_mailbox.close();
    });
    Ok(ProtocolReader {
        mailbox,
        cancellation,
        thread,
    })
}

fn cancel_protocol_reader(reader: ProtocolReader) -> thread::JoinHandle<()> {
    let ProtocolReader {
        mailbox,
        cancellation,
        thread,
    } = reader;
    // Dropping the only peer endpoint wakes the reader's poll with a hangup.
    // The reader thread alone owns adapter stdout, so no thread closes or
    // reuses a descriptor while another thread is polling or reading it.
    mailbox.cancel();
    drop(cancellation);
    thread
}

fn join_protocol_reader_thread(thread: thread::JoinHandle<()>) -> CwrResult<()> {
    thread
        .join()
        .map_err(|_| protocol_error("adapter protocol reader stopped"))?;
    Ok(())
}

fn protocol_line_unless_stopped(
    stop: &ProcessAdapterStopControl,
    received: io::Result<ProtocolDelivery>,
) -> CwrResult<Option<ProtocolDelivery>> {
    if stop.is_requested() {
        return Ok(None);
    }
    match received {
        Ok(delivery) => Ok(Some(delivery)),
        Err(error) => Err(error.into()),
    }
}

fn protocol_reader_loop(
    mut stdout: impl Read + AsRawFd,
    cancellation: UnixStream,
    mailbox: Arc<ProtocolMailbox>,
    expected_invocation_ref: String,
    expected_model_run_ref: Option<String>,
    model_output: AdapterModelOutputMailbox,
    timing: AdapterTimingSink,
) {
    let mut pending = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match wait_for_protocol_input(stdout.as_raw_fd(), cancellation.as_raw_fd()) {
            Ok(false) => return,
            Ok(true) => {}
            Err(error) => {
                let _ = mailbox.send(Err(error));
                return;
            }
        }

        match stdout.read(&mut buffer) {
            Ok(0) => {
                if !pending.is_empty() {
                    let _ = send_protocol_frame(
                        &mailbox,
                        std::mem::take(&mut pending),
                        &expected_invocation_ref,
                        expected_model_run_ref.as_deref(),
                        &model_output,
                        &timing,
                    );
                }
                return;
            }
            Ok(count) => {
                for segment in buffer[..count].split_inclusive(|byte| *byte == b'\n') {
                    if pending.len().saturating_add(segment.len()) > MAX_PROTOCOL_LINE_BYTES {
                        let _ = mailbox.send(Err(protocol_line_too_long()));
                        return;
                    }
                    pending.extend_from_slice(segment);
                    if segment.last() == Some(&b'\n')
                        && !send_protocol_frame(
                            &mailbox,
                            std::mem::take(&mut pending),
                            &expected_invocation_ref,
                            expected_model_run_ref.as_deref(),
                            &model_output,
                            &timing,
                        )
                    {
                        return;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                let _ = mailbox.send(Err(error));
                return;
            }
        }
    }
}

fn wait_for_protocol_input(stdout: libc::c_int, cancellation: libc::c_int) -> io::Result<bool> {
    let mut descriptors = [
        libc::pollfd {
            fd: stdout,
            events: libc::POLLIN | libc::POLLHUP,
            revents: 0,
        },
        libc::pollfd {
            fd: cancellation,
            events: libc::POLLIN | libc::POLLHUP,
            revents: 0,
        },
    ];
    loop {
        // SAFETY: `descriptors` is a live writable array for the supplied length, and `poll`
        // neither retains the pointer nor outlives this call.
        let ready = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                -1,
            )
        };
        if ready == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if descriptors[1].revents != 0 {
            return Ok(false);
        }
        if descriptors[0].revents & libc::POLLNVAL != 0 {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "adapter stdout descriptor became invalid",
            ));
        }
        if descriptors[0].revents != 0 {
            return Ok(true);
        }
    }
}

#[cfg(test)]
fn send_protocol_line(
    mailbox: &ProtocolMailbox,
    bytes: &[u8],
    expected_invocation_ref: &str,
    expected_model_run_ref: Option<&str>,
    model_output: &AdapterModelOutputMailbox,
    timing: &AdapterTimingSink,
) -> bool {
    send_protocol_frame(
        mailbox,
        bytes.to_vec(),
        expected_invocation_ref,
        expected_model_run_ref,
        model_output,
        timing,
    )
}

fn send_protocol_frame(
    mailbox: &ProtocolMailbox,
    bytes: Vec<u8>,
    expected_invocation_ref: &str,
    expected_model_run_ref: Option<&str>,
    model_output: &AdapterModelOutputMailbox,
    timing: &AdapterTimingSink,
) -> bool {
    if bytes.len() > MAX_PROTOCOL_LINE_BYTES {
        let _ = mailbox.send(Err(protocol_line_too_long()));
        return false;
    }
    let frame_bytes = bytes.len();
    let line = match String::from_utf8(bytes) {
        Ok(line) => line,
        Err(error) => {
            let _ = mailbox.send(Err(io::Error::new(io::ErrorKind::InvalidData, error)));
            return false;
        }
    };
    if line.trim().is_empty() {
        return true;
    }
    let parsed = serde_json::from_str::<Value>(&line);
    // Telemetry bypasses activity acknowledgements and persistence. Malformed or
    // foreign timing records are ignored rather than breaking useful work.
    if parsed
        .as_ref()
        .ok()
        .and_then(|v| v.get("type"))
        .and_then(Value::as_str)
        == Some("timing")
    {
        if frame_bytes <= 4096 {
            if let Ok(AdapterEvent::Timing {
                invocation_ref,
                model_run_ref,
                timing: record,
            }) = serde_json::from_str(&line)
            {
                if invocation_ref == expected_invocation_ref
                    && model_run_ref.as_deref() == expected_model_run_ref
                {
                    timing.record(record);
                }
            }
        }
        return true;
    }
    if parsed
        .as_ref()
        .ok()
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        == Some("model_output")
    {
        let result = parsed
            .map_err(|error| protocol_error(format!("adapter event is invalid: {error}")))
            .and_then(|value| {
                serde_json::from_value::<AdapterEvent>(value)
                    .map_err(|error| protocol_error(format!("adapter event is invalid: {error}")))
            })
            .and_then(|event| match event {
                AdapterEvent::ModelOutput {
                    invocation_ref,
                    output,
                } => model_output.accept(
                    expected_invocation_ref,
                    expected_model_run_ref,
                    invocation_ref,
                    output,
                ),
                _ => unreachable!("model_output type must deserialize as model output"),
            });
        if let Err(error) = result {
            let _ = mailbox.send(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                error.to_string(),
            )));
            return false;
        }
        return true;
    }
    if let Ok(value) = &parsed {
        match value.get("type").and_then(Value::as_str) {
            Some("result") => {
                model_output.close_run(expected_invocation_ref, expected_model_run_ref);
                mailbox.send_terminal(line);
                return false;
            }
            Some("activity") => {
                if let Ok(AdapterEvent::Activity {
                    invocation_ref,
                    event: AdapterActivityEvent::CompletedCommentary { commentary },
                    ..
                }) = serde_json::from_value::<AdapterEvent>(value.clone())
                {
                    model_output.complete_commentary(&invocation_ref, &commentary);
                }
            }
            _ => {}
        }
    }
    if let Ok(AdapterEvent::Progress {
        invocation_ref,
        sequence,
        status,
        summary,
        ..
    }) = serde_json::from_str::<AdapterEvent>(&line)
    {
        let valid = validate_event_header(
            expected_invocation_ref,
            &invocation_ref,
            sequence,
            sequence,
            &summary,
        )
        .and_then(|()| {
            let status = crate::v2026_09::invocation::ActionResponseStatus::new(status)
                .map_err(|error| protocol_error(error.to_string()))?;
            if status.is_progress() {
                Ok(())
            } else {
                protocol_failure("adapter progress status must be 1xx")
            }
        });
        if let Err(error) = valid {
            return mailbox.send(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                error.to_string(),
            )));
        }
        return mailbox.send_progress(line, sequence);
    }
    mailbox.send(Ok(ProtocolDelivery::Event {
        line,
        skipped_progress: None,
        progress_sequence: None,
    }))
}

fn protocol_line_too_long() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "adapter protocol line exceeds 32 MiB",
    )
}

include!("protocol/framing.rs");
include!("protocol/context.rs");

fn start_adapter_process(
    adapter: &ResolvedProcessAdapter,
    stop_control: &ProcessAdapterStopControl,
) -> CwrResult<AdapterProcess> {
    start_adapter_process_with_stderr_reader(adapter, stop_control, spawn_capped_stderr_reader)
}

fn start_adapter_process_with_stderr_reader(
    adapter: &ResolvedProcessAdapter,
    stop_control: &ProcessAdapterStopControl,
    start_stderr_reader: impl FnOnce(std::process::ChildStderr) -> io::Result<StderrReader>,
) -> CwrResult<AdapterProcess> {
    let mut command = Command::new(&adapter.executable);
    command
        .args(&adapter.arguments)
        .env_clear()
        .envs(&adapter.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = &adapter.current_directory {
        command.current_dir(directory);
    }
    // SAFETY: the pre-exec closure performs only the async-signal-safe `setsid` syscall and
    // constructs no state that must be shared with the parent process.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    let process_group = i32::try_from(child.id())
        .map_err(|_| protocol_error("adapter process identity is unavailable"))?;
    let mut process = AdapterProcess {
        child,
        stderr_reader: None,
        process_group,
        stop_control: stop_control.clone(),
        reaped: false,
        group_stopped: false,
    };
    let stderr = process
        .child
        .stderr
        .take()
        .ok_or_else(|| protocol_error("adapter stderr is unavailable"))?;
    // Establish cleanup ownership before any fallible reader setup.
    process.stderr_reader = Some(start_stderr_reader(stderr)?);
    if let Err(error) = stop_control.register_process_group(process_group, adapter.shutdown_grace_period) {
        let _ = process.stop();
        return Err(error);
    }
    Ok(process)
}

struct AdapterProcess {
    child: Child,
    stderr_reader: Option<StderrReader>,
    process_group: i32,
    stop_control: ProcessAdapterStopControl,
    reaped: bool,
    group_stopped: bool,
}

include!("stderr.rs");

#[derive(Clone, Copy)]
enum CleanupFailurePolicy {
    Fail,
    PreserveTerminal,
}

impl AdapterProcess {
    fn failure(
        &mut self,
        message: String,
        status: ExitStatus,
        stdout_state: &str,
    ) -> ProcessAdapterFailure {
        let stderr = self.stderr_reader.take().map(StderrReader::finish).unwrap_or_default();
        ProcessAdapterFailure {
            message,
            exit_code: status.code(),
            signal: status.signal(),
            stdout_state: stdout_state.into(),
            stderr_tail: stderr.tail,
            stderr_total_bytes: stderr.total_bytes,
            stderr_truncated: stderr.truncated,
            stderr_complete: stderr.complete,
            stderr_capture_error: stderr.error,
        }
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.try_wait_with_cleanup_policy(CleanupFailurePolicy::Fail)
    }

    fn try_wait_with_cleanup_policy(
        &mut self,
        cleanup_failure_policy: CleanupFailurePolicy,
    ) -> io::Result<Option<ExitStatus>> {
        let mut process_groups = self.stop_control.lock_process_groups();
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.reaped = true;
            if !self.group_stopped {
                if !process_groups
                    .get(&self.process_group)
                    .is_some_and(|state| state.stopped)
                {
                    handle_cleanup_result(
                        stop_adapter_process_group(self.process_group),
                        self.process_group,
                        cleanup_failure_policy,
                    )?;
                    if let Some(state) = process_groups.get_mut(&self.process_group) {
                        state.stopped = true;
                    }
                }
                self.group_stopped = true;
            }
            process_groups.remove(&self.process_group);
        }
        Ok(status)
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> CwrResult<ExitStatus> {
        self.wait_for_exit_with_cleanup_policy(timeout, CleanupFailurePolicy::Fail)
    }

    fn wait_for_exit_after_result(&mut self, timeout: Duration) -> CwrResult<ExitStatus> {
        self.wait_for_exit_with_cleanup_policy(timeout, CleanupFailurePolicy::PreserveTerminal)
    }

    fn wait_for_exit_with_cleanup_policy(
        &mut self,
        timeout: Duration,
        cleanup_failure_policy: CleanupFailurePolicy,
    ) -> CwrResult<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait_with_cleanup_policy(cleanup_failure_policy)? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return protocol_failure("adapter did not exit after returning its result");
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn stop(&mut self) -> CwrResult<()> {
        if !self.reaped && !self.stop_control.lock_process_groups().contains_key(&self.process_group) {
            stop_adapter_process_group(self.process_group)?;
        }
        self.stop_control.request_process_group_stop(self.process_group)?;
        while !self.reaped {
            // Reap under the registration lock so the escalation timer cannot
            // signal a process group whose leader pid has already been reused.
            if self.try_wait()?.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let mut process_groups = self.stop_control.lock_process_groups();
        if !self.group_stopped {
            stop_adapter_process_group(self.process_group)?;
            self.group_stopped = true;
        }
        process_groups.remove(&self.process_group);
        Ok(())
    }
}

fn handle_cleanup_result(
    result: io::Result<()>,
    process_group: i32,
    policy: CleanupFailurePolicy,
) -> io::Result<()> {
    match (result, policy) {
        (Ok(()), _) => Ok(()),
        (Err(error), CleanupFailurePolicy::Fail) => Err(error),
        (Err(error), CleanupFailurePolicy::PreserveTerminal) => {
            eprintln!(
                "cwr: adapter process group {process_group} cleanup failed after a terminal result: {error}"
            );
            Ok(())
        }
    }
}

impl Drop for AdapterProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn stop_adapter_process_group(process_group: i32) -> io::Result<()> {
    signal_adapter_process_group(process_group, libc::SIGKILL)
}

fn signal_adapter_process_group(process_group: i32, signal: i32) -> io::Result<()> {
    // SAFETY: `process_group` is the positive child pid captured after `setsid`; negating it
    // intentionally addresses that process group, and `kill` retains no Rust references.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    Ok(())
}

fn protocol_failure<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(protocol_error(message))
}

fn protocol_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message.into()).into()
}

#[cfg(test)]
#[path = "protocol/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "protocol/context_tests.rs"]
mod context_tests;
