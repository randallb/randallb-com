use cwr::adapter::{
    run_process_adapter_with_timing, AdapterOutputSinks, AdapterTimingSink, TimingCheckpoint,
    TimingDuration,
};

#[test]
fn records_startup_separating_repeated_host_acknowledgements() {
    let script = FAKE_NATIVE_TEXT_CODEX
        .replace(
            "      printf '%s\\n' '{\"id\":\"cwr-initialize\"",
            "      sleep 0.08\n      printf '%s\\n' '{\"id\":\"cwr-initialize\"",
        )
        .replace(
            "      printf '%s\\n' '{\"method\":\"item/agentMessage/delta\"",
            "      sleep 0.04\n      printf '%s\\n' '{\"method\":\"item/agentMessage/delta\"",
        );
    let (directory, fake_codex) = fixture(&script);
    fs::write(
        directory.path().join("native-text-delta-release"),
        "release",
    )
    .unwrap();
    let (sender, receiver) = mpsc::channel();
    let sink = AdapterTimingSink::new(move |record| {
        sender.send(record).unwrap();
    });
    let mut acknowledgements = 0u64;
    let outcome = run_process_adapter_with_timing(
        &codex_adapter(directory.path()),
        native_text_invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        AdapterOutputSinks {
            model_output: &AdapterModelOutputMailbox::default(),
            timing: &sink,
        },
        |_| {},
        |activity| {
            if matches!(&activity.event, AdapterActivityEvent::ProviderThread { .. })
                || matches!(&activity.event, AdapterActivityEvent::ProviderEvent { method, .. } if method == "cwr.adapter_checkpoint")
            {
                acknowledgements += 1;
                thread::sleep(Duration::from_millis(30));
            }
            Ok(())
        },
        |_| Err(std::io::Error::other("no host calls expected").into()),
    )
    .unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
    let records: Vec<_> = receiver.try_iter().collect();
    let startup = records
        .iter()
        .find(|r| r.checkpoint == TimingCheckpoint::StartupComplete)
        .unwrap();
    assert!(startup.durations_ms[&TimingDuration::ProviderInitialize] >= 70);
    assert!(startup
        .durations_ms
        .contains_key(&TimingDuration::ThreadSetup));
    assert!(startup
        .durations_ms
        .contains_key(&TimingDuration::ContextInjection));
    assert_startup_acknowledgements(&records, acknowledgements);
    let first: Vec<_> = records
        .iter()
        .filter(|r| r.checkpoint == TimingCheckpoint::FirstText)
        .collect();
    assert_eq!(first.len(), 1);
    assert!(first[0].durations_ms[&TimingDuration::FirstText] >= 30);
    assert_eq!(
        records
            .iter()
            .filter(|r| r.checkpoint == TimingCheckpoint::FirstActivity)
            .count(),
        1
    );
    assert!(records
        .iter()
        .any(|r| r.checkpoint == TimingCheckpoint::ProviderEnded));
    let wire = serde_json::to_string(&records).unwrap();
    assert!(!wire.contains("Streaming"));
    assert!(!wire.contains("please work"));
}

#[test]
fn startup_failure_records_partial_timings_without_inventing_first_output() {
    let script = "#!/bin/sh\nwhile IFS= read -r line; do\ncase \"$line\" in\n*'\"method\":\"initialize\"'*) printf '%s\\n' '{\"id\":\"cwr-initialize\",\"result\":{}}';;\n*'\"method\":\"thread/start\"'*) printf '%s\\n' '{\"id\":\"cwr-thread-start\",\"error\":{\"message\":\"failed\"}}';;\nesac\ndone\n";
    let (directory, fake_codex) = fixture(script);
    let (sender, receiver) = mpsc::channel();
    let sink = AdapterTimingSink::new(move |record| {
        sender.send(record).unwrap();
    });
    let result = run_process_adapter_with_timing(
        &codex_adapter(directory.path()),
        invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        AdapterOutputSinks {
            model_output: &AdapterModelOutputMailbox::discard(),
            timing: &sink,
        },
        |_| {},
        |_| Ok(()),
        |_| Err(std::io::Error::other("no host calls expected").into()),
    );
    let ProcessAdapterOutcome::Completed(failure) = result.expect("structured startup failure") else {
        panic!("terminal failure")
    };
    assert_eq!(failure.status, 500);
    assert_eq!(failure.summary, "Codex could not start the conversation: failed");
    let records: Vec<_> = receiver.try_iter().collect();
    assert!(records
        .iter()
        .any(|r| r.checkpoint == TimingCheckpoint::ProviderEnded));
    assert!(!records.iter().any(|r| matches!(
        r.checkpoint,
        TimingCheckpoint::FirstText
            | TimingCheckpoint::FirstActivity
            | TimingCheckpoint::StartupComplete
    )));
}

#[test]
fn first_text_arrival_excludes_prior_activity_acknowledgement_delay() {
    let (directory, fake_codex) = fixture(FAKE_NATIVE_TEXT_CODEX);
    fs::write(
        directory.path().join("native-text-delta-release"),
        "release",
    )
    .unwrap();
    let (sender, receiver) = mpsc::channel();
    let sink = AdapterTimingSink::new(move |record| sender.send(record).unwrap());
    let mut delayed = false;
    let outcome = run_process_adapter_with_timing(
        &codex_adapter(directory.path()),
        native_text_invocation(&fake_codex),
        &ProcessAdapterStopControl::new(),
        AdapterOutputSinks {
            model_output: &AdapterModelOutputMailbox::default(),
            timing: &sink,
        },
        |_| {},
        |activity| {
            if let AdapterActivityEvent::ProviderEvent { method, .. } = activity.event {
                if method == "item/started" {
                    delayed = true;
                    // The fake provider sends its delta immediately after item/started.
                    // Holding the acknowledgement delays processing, not arrival.
                    thread::sleep(Duration::from_millis(600));
                }
            }
            Ok(())
        },
        |_| Err(std::io::Error::other("no host calls expected").into()),
    )
    .unwrap();
    assert!(matches!(outcome, ProcessAdapterOutcome::Completed(_)));
    assert!(delayed);
    let records: Vec<_> = receiver.try_iter().collect();
    let startup = records
        .iter()
        .find(|r| r.checkpoint == TimingCheckpoint::StartupComplete)
        .unwrap();
    let ended = records
        .iter()
        .find(|r| r.checkpoint == TimingCheckpoint::ProviderEnded)
        .unwrap();
    assert_eq!(
        startup.durations_ms[&TimingDuration::HostAcknowledgement],
        ended.durations_ms[&TimingDuration::HostAcknowledgement],
        "live activity acknowledgements must not inflate startup time"
    );
    let duration = |checkpoint, name| {
        records
            .iter()
            .find(|record| record.checkpoint == checkpoint)
            .unwrap()
            .durations_ms[&name]
    };
    let turn_elapsed = duration(
        TimingCheckpoint::ProviderEnded,
        TimingDuration::ProviderElapsed,
    ) - duration(
        TimingCheckpoint::StartupComplete,
        TimingDuration::ProviderStartup,
    );
    let first_text = duration(TimingCheckpoint::FirstText, TimingDuration::FirstText);
    assert!(turn_elapsed.saturating_sub(first_text) >= 500,
        "first text {first_text}ms should precede the delayed acknowledgement in a {turn_elapsed}ms turn");
    assert_eq!(
        first_text,
        duration(
            TimingCheckpoint::FirstActivity,
            TimingDuration::FirstActivity
        )
    );
}

fn assert_startup_acknowledgements(records: &[cwr::adapter::AdapterTiming], acknowledgements: u64) {
    let startup = records
        .iter()
        .find(|r| r.checkpoint == TimingCheckpoint::StartupComplete)
        .unwrap();
    assert!(
        acknowledgements > 1,
        "exercise repeated history acknowledgements"
    );
    let host_ms = startup.durations_ms[&TimingDuration::HostAcknowledgement];
    assert!(host_ms >= acknowledgements * 30);
    assert!(startup
        .durations_ms
        .contains_key(&TimingDuration::ContextPreparation));
    let setup_sum: u64 = [
        TimingDuration::ContextPreparation,
        TimingDuration::ProviderInitialize,
        TimingDuration::ThreadSetup,
        TimingDuration::ContextInjection,
        TimingDuration::HostAcknowledgement,
    ]
    .iter()
    .map(|stage| startup.durations_ms[stage])
    .sum();
    assert!(
        setup_sum <= startup.durations_ms[&TimingDuration::ProviderStartup] + 2,
        "host acknowledgements must not be counted again in context injection"
    );
    let ended = records
        .iter()
        .find(|r| r.checkpoint == TimingCheckpoint::ProviderEnded)
        .unwrap();
    assert_eq!(
        ended.durations_ms[&TimingDuration::HostAcknowledgement],
        host_ms
    );
}
