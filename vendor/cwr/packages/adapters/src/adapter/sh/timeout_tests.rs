#[test]
fn command_timeout_preserves_output_from_a_live_process() {
    let event = configured_command_result(json!({
        "command": "python3 -c 'import sys,time; print(\"complete\", flush=True); print(\"diagnostic\", file=sys.stderr, flush=True); time.sleep(60)'",
        "timeout_ms": 500
    }));
    assert!(matches!(&event, AdapterEvent::Result {
        status: 504, result: None, error: Some(error), summary, ..
    } if summary == "Shell command timed out after 500 ms."
        && error.code == "command_timeout"
        && error.details.as_ref().is_some_and(|details|
            details["timed_out_after_ms"] == 500
                && details["stdout"] == "complete\n"
                && details["stderr"] == "diagnostic\n")), "{event:?}");
}

#[test]
fn run_command_input_can_override_the_configured_timeout() {
    let mut invocation = AdapterInvocation::new(
        "cwp://com.example.runtime/invocations/shell-timeout-override",
        SH_RUN_COMMAND_ACTION,
        json!({"command": "sleep 0.05; printf done", "timeout_ms": 500}),
        json!({"type": "object"}),
    );
    invocation.actor_integration_params = json!({"timeout_ms": 1});
    assert!(matches!(run_invocation(invocation), AdapterEvent::Result {
        status: 200, result: Some(result), error: None, ..
    } if result["stdout"] == "done" && result.get("timed_out_after_ms").is_none()));
}

#[test]
fn command_timeout_does_not_oversleep_a_short_deadline() {
    let started = Instant::now();
    let event = configured_command_result(json!({
        "command": "sleep 1",
        "timeout_ms": 1
    }));
    assert!(
        started.elapsed() < Duration::from_millis(80),
        "one millisecond timeout overslept: {:?}",
        started.elapsed()
    );
    assert!(matches!(event, AdapterEvent::Result {
        status: 504, error: Some(error), ..
    } if error.code == "command_timeout"));
}

#[test]
fn sustained_progress_cannot_starve_the_command_deadline() {
    let started = Instant::now();
    let event = configured_command_result(json!({
        "command": "while :; do printf 'CWR_PROGRESS\\t{\"summary\":\"still running\"}\\n' >&2; done",
        "progress_jsonl_stderr_actions": ["run--configured_action"],
        "timeout_ms": 200
    }));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "progress output starved the deadline: {:?}",
        started.elapsed()
    );
    assert!(matches!(event, AdapterEvent::Result {
        status: 504, error: Some(error), ..
    } if error.code == "command_timeout"));
}

#[test]
fn paused_host_output_does_not_block_progress_publication() {
    use std::os::fd::FromRawFd as _;

    let mut fds = [0; 2];
    // SAFETY: pipe initializes both owned descriptors on success.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: each descriptor is transferred into exactly one File.
    let _reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    // SAFETY: the write descriptor is transferred into exactly one File.
    let writer = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    // SAFETY: writer owns a live file descriptor.
    let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
    assert_ne!(flags, -1);
    assert_ne!(
        // SAFETY: writer owns a live descriptor and existing flags are preserved.
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        -1
    );
    let buffer = [b'x'; 4_096];
    loop {
        // SAFETY: buffer is valid for its full length and writer is live.
        let written = unsafe {
            libc::write(
                writer.as_raw_fd(),
                buffer.as_ptr().cast(),
                buffer.len(),
            )
        };
        if written == -1 {
            assert_eq!(std::io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock);
            break;
        }
    }
    assert_ne!(
        // SAFETY: writer is live and flags came from F_GETFL above.
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags) },
        -1
    );
    let event = AdapterEvent::Progress {
        progress: None,
        published: false,
        invocation_ref: "~@/invocations/paused-host".into(),
        sequence: 2,
        status: 102,
        summary: "still running".into(),
    };
    let started = Instant::now();
    assert!(!write_progress_frame_nonblocking(writer.as_raw_fd(), &event).unwrap());
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn progress_larger_than_the_platform_pipe_atomicity_limit_is_dropped_whole() {
    use std::io::Read as _;
    use std::os::fd::FromRawFd as _;

    let mut fds = [0; 2];
    // SAFETY: pipe initializes both owned descriptors on success.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: each descriptor is transferred into exactly one File.
    let mut reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    // SAFETY: the write descriptor is transferred into exactly one File.
    let writer = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    // SAFETY: writer owns a live pipe descriptor.
    let pipe_buf = unsafe { libc::fpathconf(writer.as_raw_fd(), libc::_PC_PIPE_BUF) };
    assert!(pipe_buf > 0);
    let event = AdapterEvent::Progress {
        progress: None,
        published: false,
        invocation_ref: "~@/invocations/oversized-progress".into(),
        sequence: 2,
        status: 102,
        summary: "x".repeat(pipe_buf as usize),
    };

    assert!(!write_progress_frame_nonblocking(writer.as_raw_fd(), &event).unwrap());
    drop(writer);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    assert!(bytes.is_empty(), "a dropped progress frame was partially written");
}

#[test]
fn deadline_worker_terminates_the_command_while_the_event_loop_is_blocked() {
    let mut child = std::process::Command::new("sh");
    child.arg("-c").arg("sleep 30").process_group(0);
    let mut child = child.spawn().unwrap();
    let started = Instant::now();
    let deadline = ShellCommandDeadline::start(child.id(), Duration::from_millis(50));

    // Stand in for a blocking host publication on the adapter's main thread.
    thread::sleep(Duration::from_millis(150));
    let status = child.wait().unwrap();

    assert!(deadline.expired());
    assert!(!status.success());
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn stop_wins_between_exit_discovery_and_the_completion_claim() {
    let _reset = ShellCommandStateReset;
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let _lifecycle = ShellCommandLifecycle::begin().unwrap();
    let mut deadline = ShellCommandDeadline::start(child.id(), Duration::from_secs(30));
    while !child_has_exited(child.id()).unwrap() {
        thread::yield_now();
    }

    let status = deadline
        .try_wait_before_completion(&mut child, || request_shell_stop(libc::SIGTERM))
        .unwrap();
    assert!(matches!(status, ShellCommandStatus::Stopped));

    deadline.cancel_and_join().unwrap();
    terminate_shell_process_group(child.id()).unwrap();
    child.wait().unwrap();
}

#[test]
fn timeout_signal_failure_is_returned_before_command_success() {
    let _reset = ShellCommandStateReset;
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let _lifecycle = ShellCommandLifecycle::begin().unwrap();
    let mut deadline = ShellCommandDeadline::start_with_signal(
        child.id(),
        Duration::from_millis(1),
        reject_timeout_signal,
    );
    while deadline.state.lock().unwrap().failure.is_none() {
        thread::yield_now();
    }
    while !child_has_exited(child.id()).unwrap() {
        thread::yield_now();
    }

    let status = deadline.try_wait(&mut child).unwrap();
    assert!(matches!(status, ShellCommandStatus::DeadlineFailed(failure)
        if failure.contains("Operation not permitted")));
    deadline.cancel_and_join().unwrap_err();
}

#[test]
fn completed_commands_win_a_short_deadline_race() {
    let _reset = ShellCommandStateReset;
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let _lifecycle = ShellCommandLifecycle::begin().unwrap();
    let stdout = ShellOutputCapture::new();
    let stderr = ShellOutputCapture::new();
    stdout.finished.store(true, Ordering::Release);
    stderr.finished.store(true, Ordering::Release);
    let (_progress_sender, progress) = std::sync::mpsc::channel();
    let (_host_input_sender, host_input) = std::sync::mpsc::channel();
    let mut output = Vec::new();
    let mut events = ShellAdapterEvents {
            gateway_operation: None,
            coworker_actions_available: true,
        input: ShellHostInput::Async(host_input, std::marker::PhantomData),
        output: &mut output,
        invocation_ref: "~@/invocations/completed-command",
        source_actor_ref: None,
        next_sequence: 2,
        progress_fd: None,
    };
    let mut deadline = ShellCommandDeadline::start(child.id(), Duration::from_secs(1));

    let result = wait_for_shell_command(
        &mut child,
        (&stdout, &stderr),
        &mut deadline,
        false,
        &progress,
        None,
        &mut events,
    )
    .unwrap();

    assert!(result.status.success());
    assert_eq!(result.timed_out_after_ms, None);
}

#[test]
fn an_expired_deadline_wins_after_try_wait_reaps_the_command() {
    let _reset = ShellCommandStateReset;
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let _lifecycle = ShellCommandLifecycle::begin().unwrap();
    let stdout = ShellOutputCapture::new();
    let stderr = ShellOutputCapture::new();
    stdout.finished.store(true, Ordering::Release);
    stderr.finished.store(true, Ordering::Release);
    let (_progress_sender, progress) = std::sync::mpsc::channel();
    let (_host_input_sender, host_input) = std::sync::mpsc::channel();
    let mut output = Vec::new();
    let mut events = ShellAdapterEvents {
            gateway_operation: None,
            coworker_actions_available: true,
        input: ShellHostInput::Async(host_input, std::marker::PhantomData),
        output: &mut output,
        invocation_ref: "~@/invocations/expired-command",
        source_actor_ref: None,
        next_sequence: 2,
        progress_fd: None,
    };
    let mut deadline = ShellCommandDeadline::start(child.id(), Duration::from_millis(1));
    while !deadline.expired() {
        thread::yield_now();
    }

    let result = wait_for_shell_command(
        &mut child,
        (&stdout, &stderr),
        &mut deadline,
        false,
        &progress,
        None,
        &mut events,
    )
    .unwrap();

    assert_eq!(result.timed_out_after_ms, Some(1));
}

#[test]
fn host_event_write_stops_at_the_command_deadline_when_the_pipe_is_full() {
    use std::os::fd::FromRawFd as _;

    let mut fds = [0; 2];
    // SAFETY: pipe initializes both owned descriptors on success.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: each descriptor is transferred into exactly one File.
    let _reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    // SAFETY: the write descriptor is transferred into exactly one File.
    let writer = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    // SAFETY: writer owns a live descriptor.
    let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
    assert_ne!(flags, -1);
    assert_ne!(
        // SAFETY: writer owns a live descriptor and existing flags are preserved.
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        -1
    );
    let buffer = [b'x'; 4_096];
    loop {
        // SAFETY: buffer is valid for its full length and writer is live.
        let written = unsafe {
            libc::write(
                writer.as_raw_fd(),
                buffer.as_ptr().cast(),
                buffer.len(),
            )
        };
        if written == -1 {
            assert_eq!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock);
            break;
        }
    }
    assert_ne!(
        // SAFETY: writer is live and flags came from F_GETFL above.
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags) },
        -1
    );

    let started = Instant::now();
    assert!(!write_frame_nonblocking_until(
        writer.as_raw_fd(),
        b"host call\n",
        started + Duration::from_millis(50),
    )
    .unwrap());
    assert!(started.elapsed() < Duration::from_millis(200));
}

#[test]
fn terminal_delivery_fails_promptly_when_the_host_pipe_stays_full() {
    use std::os::fd::FromRawFd as _;

    let mut fds = [0; 2];
    // SAFETY: pipe initializes both owned descriptors on success.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: each descriptor is transferred into exactly one File.
    let _reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    // SAFETY: the write descriptor is transferred into exactly one File.
    let writer = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    // SAFETY: writer owns a live descriptor.
    let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
    assert_ne!(flags, -1);
    assert_ne!(
        // SAFETY: writer owns a live descriptor and existing flags are preserved.
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        -1
    );
    let fill = [b'x'; 4_096];
    loop {
        // SAFETY: fill is valid for its full length and writer is live.
        let written = unsafe {
            libc::write(writer.as_raw_fd(), fill.as_ptr().cast(), fill.len())
        };
        if written == -1 {
            assert_eq!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock);
            break;
        }
    }
    assert_ne!(
        // SAFETY: writer is live and flags came from F_GETFL above.
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags) },
        -1
    );

    let started = Instant::now();
    let error = write_terminal_frame_nonblocking_until(
        writer.as_raw_fd(),
        b"timeout terminal\n",
        started + Duration::from_millis(50),
    )
    .unwrap_err();
    assert!(format!("{error:?}").contains("timed out delivering"));
    assert!(started.elapsed() < Duration::from_millis(200));
}

#[test]
fn expired_coworker_socket_deadlines_are_clean_timeouts() {
    let (mut write_client, _write_server) = CoworkerStream::pair().unwrap();
    assert!(!write_coworker_message(
        &mut write_client,
        &json!({"type":"result"}),
        Instant::now(),
    )
    .unwrap());

    let (_read_client, mut read_server) = CoworkerStream::pair().unwrap();
    let mut received = Vec::new();
    assert_eq!(
        read_coworker_message(&mut read_server, &mut received, Instant::now()).unwrap(),
        None
    );
}

#[test]
fn coworker_reader_preserves_a_coalesced_second_request() {
    use std::io::Write as _;

    let (mut client, mut server) = CoworkerStream::pair().unwrap();
    client
        .write_all(b"{\"type\":\"describe_action\"}\n{\"type\":\"invoke_action\"}\n")
        .unwrap();
    let mut received = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(1);

    assert_eq!(
        read_coworker_message(&mut server, &mut received, deadline).unwrap(),
        Some(json!({"type":"describe_action"}))
    );
    assert_eq!(
        read_coworker_message(&mut server, &mut received, deadline).unwrap(),
        Some(json!({"type":"invoke_action"}))
    );
}

#[test]
fn command_timeout_terminates_descendants_that_hold_output_pipes() {
    let event = configured_command_result(json!({
        "command": "printf parent; python3 -c 'import os,time; child=os.fork(); print(\"child\", flush=True); time.sleep(60) if child == 0 else None'",
        "timeout_ms": 500
    }));
    assert!(matches!(event, AdapterEvent::Result {
        status: 504, error: Some(error), ..
    } if error.code == "command_timeout"
        && error.details.as_ref().is_some_and(|details|
            details["exit_code"] == 0
                && details["stdout"].as_str().is_some_and(|stdout|
                    stdout.contains("parent") && stdout.contains("child")))));
}

#[test]
fn command_timeout_does_not_join_pipes_held_by_an_escaped_descendant() {
    let started = Instant::now();
    let mut invocation = AdapterInvocation::new(
        "cwp://com.example.runtime/invocations/escaped-timeout",
        "run--configured_action",
        json!({}),
        json!({"type":"object"}),
    );
    invocation.actor_integration_params = json!({
        "command": "python3 -c 'import os,time; os.setsid(); print(\"escaped\", flush=True); time.sleep(2)' &",
        "timeout_ms": 200
    });
    let invoke = AdapterHostMessage::Invoke {
        protocol: PROCESS_ADAPTER_PROTOCOL_REF.into(),
        invocation: Box::new(invocation),
    };
    let input = format!("{}\n", serde_json::to_string(&invoke).unwrap());
    let mut output = Vec::new();
    let result = run_sh_adapter(Cursor::new(input), &mut output);
    assert!(started.elapsed() < Duration::from_secs(1));
    match result {
        Ok(()) => {
            let event: AdapterEvent = serde_json::from_str(
                String::from_utf8(output).unwrap().lines().last().unwrap(),
            )
            .unwrap();
            assert!(matches!(event, AdapterEvent::Result {
                status: 504, error: Some(error), ..
            } if error.code == "command_timeout"
                && error.details.as_ref().is_some_and(|details|
                    details["stdout"] == "escaped\n")));
        }
        Err(error) => assert!(format!("{error:?}").contains("could not terminate")),
    }
}

#[test]
fn command_timeout_still_applies_after_output_pipes_close() {
    let started = Instant::now();
    let event = configured_command_result(json!({
        "command": "exec 1>&- 2>&-; sleep 2",
        "timeout_ms": 200
    }));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(matches!(event, AdapterEvent::Result {
        status: 504, error: Some(error), ..
    } if error.code == "command_timeout"));
}

#[test]
fn message_command_timeout_is_returned_as_an_error() {
    let event = message_command_result(
        json!({"message_commands":{"start":"printf partial; sleep 2"}, "timeout_ms":200}),
        "@workloop start",
    );
    assert!(matches!(event, AdapterEvent::Result {
        status: 504, result: None, error: Some(error), ..
    } if error.code == "command_timeout"
        && error.details.as_ref().is_some_and(|details| details["stdout"] == "partial")));
}

#[test]
fn coworker_bridge_stops_waiting_for_a_host_reply_at_the_deadline() {
    let mut bridge = ShellCoworkerBridge::bind(&invoke_operation()).unwrap();
    let socket = bridge.socket().to_path_buf();
    let client = thread::spawn(move || {
        let mut stream = CoworkerStream::connect(socket).unwrap();
        write_adapter_message(
            &mut stream,
            &json!({"type":"describe_action", "activation":"@helper--run--task"}),
        )
        .unwrap();
        let mut byte = [0_u8; 1];
        std::io::Read::read(&mut stream, &mut byte).ok()
    });
    let (_host_input_sender, host_input) = std::sync::mpsc::channel();
    let mut output = Vec::new();
    let mut events = ShellAdapterEvents {
            gateway_operation: None,
            coworker_actions_available: true,
        input: ShellHostInput::Async(host_input, std::marker::PhantomData),
        output: &mut output,
        invocation_ref: "~@/invocations/parent",
        source_actor_ref: None,
        next_sequence: 2,
        progress_fd: None,
    };
    let deadline = Instant::now() + Duration::from_millis(200);
    let completed = loop {
        let completed = bridge.poll(&mut events, deadline).unwrap();
        if bridge.handled {
            break completed;
        }
        thread::sleep(Duration::from_millis(5));
    };
    assert!(!completed);
    assert!(Instant::now() < deadline + Duration::from_millis(200));
    client.join().unwrap();
}

#[test]
fn coworker_bridge_rejects_an_oversized_request_frame() {
    let (_client, mut server) = CoworkerStream::pair().unwrap();
    let started = Instant::now();
    let mut received = vec![b'x'; MAX_COWORKER_MESSAGE_BYTES];
    let result = read_coworker_message(
        &mut server,
        &mut received,
        Instant::now() + Duration::from_secs(2),
    );
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
}
struct ShellCommandStateReset;

impl Drop for ShellCommandStateReset {
    fn drop(&mut self) {
        reset_shell_command_state();
    }
}

fn reject_timeout_signal(_: i32) -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(libc::EPERM))
}

fn stop_before_lifecycle_registration() {
    request_shell_command_stop();
}

fn reject_process_group_termination(_: u32) -> CwrResult<()> {
    protocol_failure("injected process-group termination failure")
}

#[test]
fn startup_stop_reaps_the_child_when_group_termination_fails() {
    let _reset = ShellCommandStateReset;
    SHELL_BEFORE_LIFECYCLE_HOOK.set(Some(stop_before_lifecycle_registration));
    SHELL_TERMINATE_OVERRIDE.set(Some(reject_process_group_termination));
    let marker_dir = tempfile::tempdir().unwrap();
    let marker = marker_dir.path().join("child-finished");
    let command = format!("sleep 0.05; printf finished > {}", marker.display());

    let mut invocation = AdapterInvocation::new(
        "cwp://com.example.runtime/invocations/startup-stop-cleanup",
        "run--configured_action",
        json!({}),
        json!({"type":"object"}),
    );
    invocation.actor_integration_params = json!({"command": command});
    let invoke = AdapterHostMessage::Invoke {
        protocol: PROCESS_ADAPTER_PROTOCOL_REF.into(),
        invocation: Box::new(invocation),
    };
    let input = format!("{}\n", serde_json::to_string(&invoke).unwrap());
    let mut output = Vec::new();

    let error = run_sh_adapter(Cursor::new(input), &mut output).unwrap_err();

    assert!(format!("{error:?}").contains("injected process-group termination failure"));
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "finished");
}
