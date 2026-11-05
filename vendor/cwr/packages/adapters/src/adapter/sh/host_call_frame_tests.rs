#[test]
fn host_events_larger_than_pipe_buf_complete_after_partial_writes() {
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
    let frame = vec![b'h'; pipe_buf as usize * 128];
    let expected = frame.clone();
    let write = thread::spawn(move || {
        write_frame_nonblocking_until(
            writer.as_raw_fd(),
            &frame,
            Instant::now() + Duration::from_secs(2),
        )
    });

    // Let the first nonblocking write fill the pipe before draining it.
    thread::sleep(Duration::from_millis(50));
    let mut bytes = vec![0; expected.len()];
    reader.read_exact(&mut bytes).unwrap();
    assert!(write.join().unwrap().unwrap());
    assert_eq!(bytes, expected);
}

#[test]
fn a_stalled_partial_host_frame_fails_without_hanging() {
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
    fill_pipe_except_for_one_atomic_write(&mut reader, &writer, pipe_buf as usize);
    let frame = vec![b'h'; pipe_buf as usize * 4];
    let started = Instant::now();

    let error = write_frame_nonblocking_until(
        writer.as_raw_fd(),
        &frame,
        started + Duration::from_millis(20),
    )
    .unwrap_err();
    assert!(format!("{error:?}").contains("timed out completing"));
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn a_stop_during_a_partial_host_frame_fails_without_appending_output() {
    use std::os::fd::FromRawFd as _;

    let _reset = ShellCommandStateReset;
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
    fill_pipe_except_for_one_atomic_write(&mut reader, &writer, pipe_buf as usize);
    SHELL_HOST_FRAME_AFTER_FIRST_WRITE_HOOK
        .set(Some(stop_after_first_host_frame_write));

    let error = write_frame_nonblocking_until(
        writer.as_raw_fd(),
        &vec![b'h'; pipe_buf as usize * 4],
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap_err();
    assert!(format!("{error:?}").contains("stopped while completing"));
}

fn stop_after_first_host_frame_write() {
    request_shell_stop(libc::SIGTERM);
}

fn fill_pipe_except_for_one_atomic_write(
    reader: &mut std::fs::File,
    writer: &std::fs::File,
    pipe_buf: usize,
) {
    use std::io::Read as _;

    // SAFETY: writer owns a live descriptor.
    let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
    assert_ne!(flags, -1);
    assert_ne!(
        // SAFETY: writer is live and the existing flags are preserved.
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        -1
    );
    let fill = [b'x'; 4_096];
    loop {
        // SAFETY: fill is valid for its full length and writer is live.
        let written = unsafe { libc::write(writer.as_raw_fd(), fill.as_ptr().cast(), fill.len()) };
        if written == -1 {
            assert_eq!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock);
            break;
        }
    }
    let mut released = vec![0; pipe_buf];
    reader.read_exact(&mut released).unwrap();
    assert_ne!(
        // SAFETY: writer is live and flags came from F_GETFL above.
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags) },
        -1
    );
}

#[test]
fn full_length_coworker_targets_and_large_inputs_reach_the_host() {
    use std::io::BufRead as _;
    use std::os::fd::FromRawFd as _;

    let mut fds = [0; 2];
    // SAFETY: pipe initializes both owned descriptors on success.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: each descriptor is transferred into exactly one File.
    let reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    // SAFETY: the write descriptor is transferred into exactly one File.
    let mut writer = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    // SAFETY: writer owns a live pipe descriptor.
    let pipe_buf = unsafe { libc::fpathconf(writer.as_raw_fd(), libc::_PC_PIPE_BUF) };
    assert!(pipe_buf > 0);
    let (host_input_sender, host_input) = std::sync::mpsc::channel();
    let writer_fd = writer.as_raw_fd();
    let invocation_ref = format!("cwp://runtime.example/invocations/{}", "i".repeat(256));
    let response_ref = invocation_ref.clone();
    let host = thread::spawn(move || {
        let mut line = String::new();
        std::io::BufReader::new(reader).read_line(&mut line).unwrap();
        let event: AdapterEvent = serde_json::from_str(&line).unwrap();
        host_input_sender
            .send(Ok(Some(AdapterHostMessage::CallResult {
                invocation_ref: response_ref,
                call_id: SHELL_ACTION_CALL_ID.into(),
                result: Some(json!({
                    "invocation_ref":"~@/invocations/child",
                    "sequence":2,
                    "status":200,
                    "summary":"Finished.",
                    "result":{"ok":true}
                })),
                error: None,
            })))
            .unwrap();
        event
    });
    let mut events = ShellAdapterEvents {
            gateway_operation: None,
            coworker_actions_available: true,
        input: ShellHostInput::Async(host_input, std::marker::PhantomData),
        output: &mut writer,
        invocation_ref: &invocation_ref,
        source_actor_ref: None,
        next_sequence: 2,
        progress_fd: Some(writer_fd),
    };

    let response = events
        .invoke_action(
            json!({
                "action_name":"run--task",
                "binding_ref":format!("cwp://org.example/resources/{}/bindings/local_sh", "b".repeat(128)),
                "contract_id":format!("sha256:{}", "c".repeat(64)),
                "coworker_ref":format!("cwp://org.example/resources/{}", "w".repeat(128)),
                "role_ref":format!("cwp://org.example/resources/{}", "r".repeat(128)),
                "runtime_ref":format!("cwp://runtime.example/runtimes/{}", "t".repeat(128))
            }),
            Value::String("x".repeat(MAX_ACTION_VALUE_BYTES - 2)),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap()
        .unwrap();
    assert_eq!(response["type"], "completed");
    let event = host.join().unwrap();
    assert!(matches!(event, AdapterEvent::Call { input, .. }
        if serde_json::to_vec(&input["input"]).unwrap().len() == MAX_ACTION_VALUE_BYTES));
}

#[test]
fn a_stop_before_a_host_frame_starts_leaves_stdout_clean() {
    use std::io::Read as _;
    use std::os::fd::FromRawFd as _;

    let _reset = ShellCommandStateReset;
    let mut fds = [0; 2];
    // SAFETY: pipe initializes both owned descriptors on success.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: each descriptor is transferred into exactly one File.
    let mut reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    // SAFETY: the write descriptor is transferred into exactly one File.
    let writer = unsafe { std::fs::File::from_raw_fd(fds[1]) };
    request_shell_stop(libc::SIGTERM);

    assert!(!write_frame_nonblocking_until(
        writer.as_raw_fd(),
        b"host call\n",
        Instant::now() + Duration::from_secs(1),
    )
    .unwrap());
    drop(writer);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    assert!(bytes.is_empty());
}
