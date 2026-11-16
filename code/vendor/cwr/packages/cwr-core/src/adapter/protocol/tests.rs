use super::*;

#[test]
fn timings_bypass_acknowledgements_and_reject_foreign_or_malformed_records() {
    let protocol = ProtocolMailbox::new();
    let records = Arc::new(Mutex::new(Vec::new()));
    let observed = records.clone();
    let sink = AdapterTimingSink::new(move |record| observed.lock().unwrap().push(record));
    let event = AdapterEvent::Timing {
        invocation_ref: "cwp://example.test/invocations/one".into(),
        model_run_ref: Some("cwp://example.test/model_runs/one".into()),
        timing: AdapterTiming::single(TimingCheckpoint::FirstText, TimingDuration::FirstText, 150),
    };
    let bytes = serde_json::to_vec(&event).unwrap();
    for expected in [
        "cwp://example.test/invocations/one",
        "cwp://example.test/invocations/other",
    ] {
        assert!(send_protocol_line(
            &protocol,
            &bytes,
            expected,
            Some("cwp://example.test/model_runs/one"),
            &AdapterModelOutputMailbox::discard(),
            &sink
        ));
    }
    assert!(send_protocol_line(
        &protocol,
        &bytes,
        "cwp://example.test/invocations/one",
        Some("cwp://example.test/model_runs/old"),
        &AdapterModelOutputMailbox::discard(),
        &sink
    ));
    assert!(send_protocol_line(
        &protocol,
        br#"{"type":"timing","timing":{"secret":"ignored"}}"#,
        "cwp://example.test/invocations/one",
        None,
        &AdapterModelOutputMailbox::discard(),
        &sink
    ));
    assert_eq!(records.lock().unwrap().len(), 1);
    assert!(
        protocol.try_recv().is_err(),
        "timings must not reach the activity/ack loop"
    );
}

#[test]
fn terminal_delivery_records_progress_dropped_after_the_queue_fills() {
    let protocol = ProtocolMailbox::new();
    protocol.set_progress_coalescing(true);
    let invocation_ref = "cwp://example.test/invocations/one";
    for sequence in 1..=10 {
        let progress = json!({
            "type": "progress",
            "invocation_ref": invocation_ref,
            "sequence": sequence,
            "status": 102,
            "summary": format!("progress {sequence}"),
        });
        assert!(send_protocol_line(
            &protocol,
            &serde_json::to_vec(&progress).unwrap(),
            invocation_ref,
            None,
            &AdapterModelOutputMailbox::discard(),
            &AdapterTimingSink::default(),
        ));
    }
    let terminal = json!({
        "type": "result",
        "invocation_ref": invocation_ref,
        "sequence": 11,
        "status": 200,
        "summary": "done",
        "result": {},
    });
    assert!(!send_protocol_line(
        &protocol,
        &serde_json::to_vec(&terminal).unwrap(),
        invocation_ref,
        None,
        &AdapterModelOutputMailbox::discard(),
        &AdapterTimingSink::default(),
    ));

    for sequence in 1..=PROTOCOL_QUEUE_FRAMES as u64 {
        let ProtocolDelivery::Event {
            line,
            skipped_progress,
            ..
        } = protocol.try_recv().unwrap().unwrap()
        else {
            panic!("queued progress must remain ordered")
        };
        let AdapterEvent::Progress {
            sequence: actual, ..
        } = serde_json::from_str(&line).unwrap()
        else {
            panic!("queued event must remain progress")
        };
        if sequence < PROTOCOL_QUEUE_FRAMES as u64 {
            assert_eq!(actual, sequence);
            assert_eq!(skipped_progress, None);
        } else {
            assert_eq!(actual, 10);
            assert_eq!(skipped_progress, Some((4, 9)));
        }
    }
    let ProtocolDelivery::Terminal {
        line,
        skipped_progress,
    } = protocol.try_recv().unwrap().unwrap()
    else {
        panic!("terminal must be retained independently")
    };
    assert_eq!(skipped_progress, None);
    assert!(matches!(
        serde_json::from_str::<AdapterEvent>(&line).unwrap(),
        AdapterEvent::Result { sequence: 11, .. }
    ));
}

#[test]
fn ordinary_queue_enforces_encoded_bytes_while_terminal_stays_independent() {
    let protocol = ProtocolMailbox::new();
    let first = "a".repeat(20 * 1024 * 1024);
    assert!(protocol.send(Ok(ProtocolDelivery::Event {
        line: first,
        skipped_progress: None,
        progress_sequence: None,
    })));
    {
        let state = protocol.state.lock().unwrap();
        assert_eq!(state.queued_bytes, 20 * 1024 * 1024);
        assert!(!can_enqueue(&state, 13 * 1024 * 1024));
        assert!(can_enqueue(&state, 12 * 1024 * 1024));
    }
    protocol.send_terminal("t".repeat(MAX_PROTOCOL_LINE_BYTES));
    let state = protocol.state.lock().unwrap();
    assert_eq!(state.queued_bytes, 20 * 1024 * 1024);
    assert!(state.terminal.is_some());
}

#[test]
fn cancellation_releases_a_sender_blocked_by_the_queue_byte_budget() {
    let protocol = Arc::new(ProtocolMailbox::new());
    assert!(protocol.send(Ok(ProtocolDelivery::Event {
        line: "a".repeat(20 * 1024 * 1024),
        skipped_progress: None,
        progress_sequence: None,
    })));
    let blocked = Arc::clone(&protocol);
    let (started, wait_for_start) = mpsc::channel();
    let sender = thread::spawn(move || {
        started.send(()).unwrap();
        blocked.send(Ok(ProtocolDelivery::Event {
            line: "b".repeat(13 * 1024 * 1024),
            skipped_progress: None,
            progress_sequence: None,
        }))
    });
    wait_for_start.recv().unwrap();
    thread::sleep(Duration::from_millis(20));
    protocol.cancel();
    assert!(!sender.join().unwrap());
}

#[test]
fn cleanup_failure_after_terminal_result_is_nonfatal() {
    let error = io::Error::from_raw_os_error(libc::EPERM);

    handle_cleanup_result(Err(error), 42, CleanupFailurePolicy::PreserveTerminal)
        .expect("a validated terminal result must survive cleanup failure");
}

#[test]
fn cleanup_failure_without_terminal_result_remains_fatal() {
    let error = io::Error::from_raw_os_error(libc::EPERM);

    let returned = handle_cleanup_result(Err(error), 42, CleanupFailurePolicy::Fail)
        .expect_err("cleanup failure before a terminal result must be returned");
    assert_eq!(returned.raw_os_error(), Some(libc::EPERM));
}

#[test]
fn stderr_setup_failure_kills_and_reaps_adapter() {
    let adapter = ResolvedProcessAdapter::new("/bin/sh")
        .with_argument("-c")
        .with_argument("echo $$ >&2; exec /bin/sleep 60");
    let stop = ProcessAdapterStopControl::new();
    let mut pid = 0;
    let result = start_adapter_process_with_stderr_reader(&adapter, &stop, |stderr| {
        let mut line = String::new();
        io::BufReader::new(stderr).read_line(&mut line).unwrap();
        pid = line.trim().parse::<libc::pid_t>().unwrap();
        Err(io::Error::from_raw_os_error(libc::EMFILE))
    });
    let error = result.err().expect("stderr setup must fail");
    assert_eq!(error.downcast_ref::<io::Error>().unwrap().raw_os_error(), Some(libc::EMFILE));
    assert!(stop.lock_process_groups().is_empty());
    let mut status = 0;
    // SAFETY: pid identifies our child and status is writable for this call.
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) }, -1);
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ECHILD));
}
