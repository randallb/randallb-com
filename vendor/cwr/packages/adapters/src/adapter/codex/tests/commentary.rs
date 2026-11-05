#[test]
fn codex_adapter_waits_for_provider_thread_retention_before_starting_turn() {
    let (directory, fake_codex) = fixture(FAKE_ACTIVITY_GATE_CODEX);
    let error = run_process_adapter_with_activity_and_host_calls(
        &codex_adapter(directory.path()),
        invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        |_| {},
        |_| Err(std::io::Error::other("provider thread retention failed").into()),
        |_| Err(std::io::Error::other("host calls are unavailable").into()),
    )
    .expect_err("rejected provider thread activity must fail the adapter");
    assert!(error
        .to_string()
        .contains("provider thread retention failed"));
    assert!(!directory.path().join("turn-started").exists());
}

#[test]
fn codex_adapter_delivers_finished_commentary_before_the_action_completes() {
    let (directory, fake_codex) = fixture(FAKE_COMMENTARY_GATE_CODEX);
    let adapter = codex_adapter(directory.path());
    let requested = invocation(&fake_codex);
    let release = directory.path().join("commentary-release");
    let delta_release = directory.path().join("commentary-delta-release");
    let reached_adapter = directory.path().join("commentary-reached-adapter");
    let (commentary_sender, commentary_receiver) = mpsc::channel();
    let (outcome_sender, outcome_receiver) = mpsc::channel();
    let stop = ProcessAdapterStopControl::new();
    let runner_stop = stop.clone();
    let runner = thread::spawn(move || {
        let outcome = run_process_adapter_with_activity_and_host_calls(
            &adapter,
            requested,
            &runner_stop,
            |_| {},
            |activity| {
                if let AdapterActivityEvent::CompletedCommentary { commentary } = activity.event {
                    commentary_sender.send(commentary).unwrap();
                }
                Ok(())
            },
            |_| Err(std::io::Error::other("host calls are unavailable").into()),
        );
        outcome_sender.send(outcome).unwrap();
    });

    if !wait_for_fixture_marker(&reached_adapter) {
        stop.stop();
        let _ = fs::write(&delta_release, b"release");
        let _ = fs::write(&release, b"release");
        panic!(
            "fixture marker did not appear at {}",
            reached_adapter.display()
        );
    }
    fs::write(&delta_release, b"release").unwrap();
    let commentary = commentary_receiver.recv_timeout(Duration::from_secs(1));
    assert!(!release.exists());
    fs::write(&release, b"release").unwrap();
    if commentary.is_err() {
        stop.stop();
    }

    let outcome = outcome_receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|error| {
            stop.stop();
            panic!("adapter runner did not finish after release: {error}")
        })
        .expect("adapter must complete after the commentary gate is released");
    runner.join().expect("adapter runner must not panic");
    let commentary =
        commentary.expect("completed commentary must reach the host within one second");
    assert_eq!(commentary.item_id, "commentary-gate-item");
    assert_eq!(commentary.text, "Visible before completion.");
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
}

#[test]
fn codex_adapter_streams_scoped_commentary_before_the_turn_start_response_without_an_ack() {
    let (directory, fake_codex) = fixture(FAKE_COMMENTARY_GATE_CODEX);
    let adapter = codex_adapter(directory.path());
    let requested = invocation(&fake_codex);
    let delta_emitted = directory.path().join("commentary-delta-emitted");
    let delta_release = directory.path().join("commentary-delta-release");
    let completion_release = directory.path().join("commentary-release");
    let mailbox = AdapterModelOutputMailbox::default();
    let runner_mailbox = mailbox.clone();
    let stop = ProcessAdapterStopControl::new();
    let runner_stop = stop.clone();
    let (outcome_sender, outcome_receiver) = mpsc::channel();
    let runner = thread::spawn(move || {
        let outcome = run_process_adapter_with_model_output_activity_and_controlled_host_calls(
            &adapter,
            requested,
            &runner_stop,
            &runner_mailbox,
            |_| {},
            |_| Ok(()),
            |_| Err(std::io::Error::other("host calls are unavailable").into()),
        );
        outcome_sender.send(outcome).unwrap();
    });

    if !wait_for_fixture_marker(&delta_emitted) {
        stop.stop();
        let _ = fs::write(&delta_release, b"release");
        let _ = fs::write(&completion_release, b"release");
        let _ = outcome_receiver.recv_timeout(Duration::from_secs(5));
        let _ = runner.join();
        panic!("fixture did not emit the gated commentary delta");
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    let streamed = loop {
        let snapshot = mailbox.snapshot();
        if snapshot.output.is_some() || Instant::now() >= deadline {
            break snapshot;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let Some(commentary) = streamed.output else {
        stop.stop();
        let _ = fs::write(&delta_release, b"release");
        let _ = fs::write(&completion_release, b"release");
        let _ = outcome_receiver.recv_timeout(Duration::from_secs(5));
        let _ = runner.join();
        panic!("commentary delta must cross the adapter before turn/start returns");
    };
    let streamed_without_ack = !delta_release.exists();
    fs::write(&delta_release, b"release").unwrap();
    fs::write(&completion_release, b"release").unwrap();
    assert!(streamed_without_ack);
    assert_eq!(commentary.provider, "codex");
    assert_eq!(
        commentary.thread_id.as_deref(),
        Some("commentary-gate-thread")
    );
    assert_eq!(commentary.response_or_turn_id, "commentary-gate-turn");
    assert_eq!(commentary.item_id, "commentary-gate-item");
    assert_eq!(commentary.text, "Visible before completion.");

    let outcome = outcome_receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|error| {
            stop.stop();
            panic!("adapter runner did not finish after release: {error}")
        })
        .expect("adapter must complete after both commentary gates are released");
    runner.join().expect("adapter runner must not panic");
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
    assert!(mailbox.snapshot().output.is_none());
}

