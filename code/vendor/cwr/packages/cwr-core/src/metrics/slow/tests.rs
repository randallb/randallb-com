use super::*;
use std::fs;

fn sample(at: Instant, state: ActionInvocationState) -> Sample {
    Sample {
        context: super::super::tests::context(1),
        conversation_ref: Some("cwp://example.test/conversations/one".into()),
        source_message_ref: Some("cwp://example.test/messages/one".into()),
        state,
        observed: at,
        recovered_age: None,
        attempt_active: false,
        started: state == ActionInvocationState::Running,
    }
}

#[test]
fn queued_work_is_logged_without_execution_then_finishes_once() {
    let start = Instant::now();
    let mut monitor = Monitor::new(Duration::from_secs(30));
    let mut records = Vec::new();
    let drops = AtomicU64::new(0);
    monitor.update(
        sample(start, ActionInvocationState::Admitted),
        &mut records,
        &drops,
    );
    monitor.tick(start + Duration::from_secs(29), &mut records);
    assert!(records.is_empty());
    monitor.tick(start + Duration::from_secs(30), &mut records);
    monitor.tick(start + Duration::from_secs(31), &mut records);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].measurements["state"], "admitted");
    assert_eq!(records[0].measurements["elapsed_ms"], 30_000);
    let mut running = sample(
        start + Duration::from_secs(32),
        ActionInvocationState::Running,
    );
    running.source_message_ref = None;
    monitor.update(running, &mut records, &drops);
    let mut end = sample(
        start + Duration::from_secs(40),
        ActionInvocationState::Completed,
    );
    end.started = true;
    end.source_message_ref = None;
    monitor.update(end, &mut records, &drops);
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].checkpoint, "slow_invocation_ended");
    assert_eq!(records[1].measurements["elapsed_ms"], 40_000);
    assert_eq!(records[1].measurements["queue_ms"], 32_000);
    assert_eq!(records[1].measurements["outcome"], "completed");
    assert_eq!(
        records[1].measurements["source_message_ref"],
        "cwp://example.test/messages/one"
    );
    assert!(monitor.tracked.is_empty());
}

#[test]
fn fast_work_is_silent_and_every_terminal_outcome_is_preserved() {
    for state in [
        ActionInvocationState::Completed,
        ActionInvocationState::Failed,
        ActionInvocationState::Blocked,
        ActionInvocationState::Stopped,
    ] {
        for seconds in [1, 35] {
            let start = Instant::now();
            let mut monitor = Monitor::new(Duration::from_secs(30));
            let mut records = Vec::new();
            let drops = AtomicU64::new(0);
            monitor.update(
                sample(start, ActionInvocationState::Admitted),
                &mut records,
                &drops,
            );
            monitor.update(
                sample(start + Duration::from_secs(seconds), state),
                &mut records,
                &drops,
            );
            assert_eq!(records.len(), if seconds == 1 { 0 } else { 2 });
            if seconds == 35 {
                assert_eq!(records[1].measurements["outcome"], json!(state));
            }
            assert!(monitor.tracked.is_empty());
        }
    }
}

#[test]
fn recovery_is_explicitly_approximate_and_missing_admission_is_not_fabricated() {
    let start = Instant::now();
    let mut monitor = Monitor::new(Duration::from_secs(30));
    let mut records = Vec::new();
    let drops = AtomicU64::new(0);
    monitor.update(
        sample(start, ActionInvocationState::Stopped),
        &mut records,
        &drops,
    );
    assert!(records.is_empty());
    let mut recovered = sample(start, ActionInvocationState::Running);
    recovered.recovered_age = Some(Duration::from_secs(60));
    monitor.update(recovered, &mut records, &drops);
    monitor.tick(start, &mut records);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].measurements["elapsed_is_approximate"], true);
    assert_eq!(records[0].measurements["elapsed_ms"], 60_000);
    assert!(records[0].measurements["queue_ms"].is_null());
}

#[test]
fn tracked_memory_is_bounded() {
    let start = Instant::now();
    let mut monitor = Monitor::new(Duration::from_secs(30));
    let mut records = Vec::new();
    let drops = AtomicU64::new(0);
    for id in 0..=MAX_TRACKED {
        let mut next = sample(start, ActionInvocationState::Admitted);
        next.context.invocation_ref = format!("cwp://example.test/invocations/{id}");
        monitor.update(next, &mut records, &drops);
    }
    assert_eq!(monitor.tracked.len(), MAX_TRACKED);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn running_stop_waits_for_provider_cleanup() {
    let start = Instant::now();
    let mut monitor = Monitor::new(Duration::from_secs(30));
    let mut records = Vec::new();
    let drops = AtomicU64::new(0);
    monitor.update(
        sample(start, ActionInvocationState::Admitted),
        &mut records,
        &drops,
    );
    let mut stopped = sample(
        start + Duration::from_secs(31),
        ActionInvocationState::Stopped,
    );
    stopped.attempt_active = true;
    monitor.update(stopped, &mut records, &drops);
    monitor.tick(start + Duration::from_secs(31), &mut records);
    assert_eq!(records.len(), 1);
    assert!(records[0].measurements["outcome"].is_null());
    monitor.update(
        sample(
            start + Duration::from_secs(35),
            ActionInvocationState::Stopped,
        ),
        &mut records,
        &drops,
    );
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].measurements["elapsed_ms"], 35_000);
    assert_eq!(records[1].measurements["outcome"], "stopped");
}

#[test]
fn delayed_writer_drains_completed_work_before_ticking() {
    let directory = tempfile::tempdir().unwrap();
    let (send, receive) = mpsc::sync_channel(QUEUE_CAPACITY);
    let start = Instant::now() - Duration::from_secs(60);
    send.send(WriterMessage::Invocation(Box::new(sample(
        start,
        ActionInvocationState::Admitted,
    ))))
    .unwrap();
    send.send(WriterMessage::Invocation(Box::new(sample(
        start + Duration::from_secs(1),
        ActionInvocationState::Completed,
    ))))
    .unwrap();
    drop(send);
    run_writer(
        &directory.path().join("metrics.sqlite3"),
        receive,
        &AtomicU64::new(0),
        Duration::from_secs(30),
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn recovered_cancellation_records_approximate_settled_duration() {
    let start = Instant::now();
    let mut monitor = Monitor::new(Duration::from_secs(30));
    let mut records = Vec::new();
    let drops = AtomicU64::new(0);
    let mut recovered = sample(start, ActionInvocationState::Stopped);
    recovered.attempt_active = true;
    recovered.recovered_age = Some(Duration::from_secs(60));
    monitor.update(recovered, &mut records, &drops);
    monitor.update(
        sample(
            start + Duration::from_secs(1),
            ActionInvocationState::Stopped,
        ),
        &mut records,
        &drops,
    );
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].measurements["elapsed_is_approximate"], true);
    assert_eq!(records[1].measurements["elapsed_ms"], 61_000);
    assert_eq!(records[1].measurements["outcome"], "stopped");
}

#[test]
fn busy_writer_ticks_before_consuming_the_next_batch() {
    let directory = tempfile::tempdir().unwrap();
    let (send, receive) = mpsc::sync_channel(QUEUE_CAPACITY + 1);
    let start = Instant::now() - Duration::from_secs(60);
    send.send(WriterMessage::Invocation(Box::new(sample(
        start,
        ActionInvocationState::Admitted,
    ))))
    .unwrap();
    for _ in 1..QUEUE_CAPACITY {
        send.send(WriterMessage::Invocation(Box::new(sample(
            start + Duration::from_secs(31),
            ActionInvocationState::Admitted,
        ))))
        .unwrap();
    }
    send.send(WriterMessage::Invocation(Box::new(sample(
        start + Duration::from_secs(40),
        ActionInvocationState::Completed,
    ))))
    .unwrap();
    drop(send);
    run_writer(
        &directory.path().join("metrics.sqlite3"),
        receive,
        &AtomicU64::new(0),
        Duration::from_secs(30),
    );
    let rows = crate::metrics::tests::saved_records(directory.path());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["checkpoint"], "slow_invocation_threshold");
    assert_eq!(rows[0]["measurements"]["state"], "admitted");
    assert_eq!(rows[0]["measurements"]["elapsed_ms"], 31_000);
    assert_eq!(rows[1]["measurements"]["elapsed_ms"], 40_000);
}
