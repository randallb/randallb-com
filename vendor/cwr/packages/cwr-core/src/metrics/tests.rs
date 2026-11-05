use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

pub(super) fn saved_records(root: &std::path::Path) -> Vec<Value> {
    let mut store =
        cwpdb_store::NativeStore::open_read_only(root.join(LOCAL_METRICS_DATABASE_FILENAME))
            .unwrap();
    let page = store.metric_records(None, 10_000).unwrap();
    assert!(!page.truncated);
    page.records
}

pub(super) fn context(attempt: u64) -> MetricContext {
    MetricContext {
        runtime_ref: "cwp://example.test/runtime".into(),
        invocation_ref: "cwp://example.test/invocations/one".into(),
        conversation_ref: Some("cwp://example.test/conversations/one".into()),
        parent_invocation_ref: None,
        attempt,
        model_run_ref: Some(format!("cwp://example.test/model_runs/{attempt}")),
        actor_ref: "~@/actors/helper".into(),
        model: Some("test-model".into()),
        reasoning_effort: Some("low".into()),
        invocation_continuation_supplied: false,
        private_history_supplied: false,
        context_bytes: 42,
    }
}

#[test]
fn recording_is_separate_private_and_preserves_attempts_and_missing_values() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("conversation-admission.json");
    fs::write(&state, "unchanged").unwrap();
    let recorder = MetricsRecorder::new(&state);
    for attempt in [1, 2] {
        let mut metric_context = context(attempt);
        if attempt == 1 {
            metric_context.private_history_supplied = true;
        } else {
            metric_context.invocation_continuation_supplied = true;
        }
        let guard = AttemptMetrics {
            recorder: recorder.clone(),
            context: metric_context,
            started: Instant::now(),
            outcome: "failed".into(),
            total_turn_ms: None,
        };
        recorder.record(
            &guard.context,
            "runtime",
            "attempt_started",
            json!({"queue_ms": null}),
        );
        drop(guard);
    }
    recorder.flush();
    let path = directory.path().join(LOCAL_METRICS_DATABASE_FILENAME);
    let rows = saved_records(directory.path());
    let text = serde_json::to_string(&rows).unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["schema_version"], 2);
    assert_eq!(rows[0]["attempt"], 1);
    assert_eq!(rows[2]["attempt"], 2);
    assert_eq!(rows[0]["private_history_supplied"], true);
    assert_eq!(rows[0]["invocation_continuation_supplied"], false);
    assert_eq!(rows[2]["private_history_supplied"], false);
    assert_eq!(rows[2]["invocation_continuation_supplied"], true);
    assert!(rows[0].get("thread_reused").is_none());
    assert!(rows[0]["measurements"]["queue_ms"].is_null());
    assert_eq!(rows[1]["measurements"]["outcome"], "failed");
    assert!(!text.contains("first_text"));
    assert_eq!(fs::read_to_string(state).unwrap(), "unchanged");
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn unavailable_storage_and_full_queue_only_drop_metrics() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join(LOCAL_METRICS_DATABASE_FILENAME)).unwrap();
    let recorder = MetricsRecorder::new(&directory.path().join("state.json"));
    recorder.record(&context(1), "runtime", "attempt_started", json!({}));
    recorder.flush();
    assert_eq!(recorder.dropped.load(Ordering::Relaxed), 1);

    let (sender, _blocked_receiver) = mpsc::sync_channel(1);
    let (_, finished) = mpsc::channel();
    let recorder = MetricsRecorder {
        writer: Arc::new(MetricsWriter {
            sender: Some(sender),
            finished: Mutex::new(finished),
        }),
        dropped: Arc::new(AtomicU64::new(0)),
    };
    recorder.record(&context(1), "runtime", "attempt_started", json!({}));
    recorder.record(&context(1), "runtime", "attempt_ended", json!({}));
    assert_eq!(recorder.dropped.load(Ordering::Relaxed), 1);
}

#[test]
fn process_failure_records_only_safe_diagnostics() {
    let directory = tempfile::tempdir().unwrap();
    let recorder = MetricsRecorder::new(&directory.path().join("state.json"));
    let secret = "provider output with /private/path and credential";
    let error: Box<dyn std::error::Error + Send + Sync> =
        Box::new(crate::adapter::ProcessAdapterFailure {
            message: "adapter exited before returning a result".into(),
            exit_code: Some(23),
            signal: None,
            stdout_state: "eof".into(),
            stderr_tail: secret.into(),
            stderr_total_bytes: 47,
            stderr_truncated: true,
            stderr_complete: true,
            stderr_capture_error: None,
        });

    recorder.record_adapter_process_failure(&context(1), error.as_ref());
    recorder.flush();

    let rows = saved_records(directory.path());
    let row = &rows[0];
    let text = serde_json::to_string(&rows).unwrap();
    assert_eq!(row["checkpoint"], "adapter_process_failed");
    assert_eq!(row["producer"], "process_transport");
    assert_eq!(row["measurements"]["phase"], "awaiting_result");
    assert_eq!(row["measurements"]["exit_code"], 23);
    assert!(row["measurements"]["signal"].is_null());
    assert_eq!(row["measurements"]["stdout_state"], "eof");
    assert_eq!(row["measurements"]["stderr_total_bytes"], 47);
    assert_eq!(row["measurements"]["stderr_truncated"], true);
    assert!(!text.contains(secret));
    assert!(!text.contains("stderr_tail"));
}

#[test]
fn adapter_compaction_timing_reaches_the_best_effort_metrics_store() {
    let directory = tempfile::tempdir().unwrap();
    let recorder = MetricsRecorder::new(&directory.path().join("state.json"));
    let sink = recorder.adapter_sink(context(1), Instant::now());
    sink.record(crate::adapter::AdapterTiming {
        checkpoint: crate::adapter::TimingCheckpoint::PreparationCompactionCompleted,
        durations_ms: std::collections::BTreeMap::from([(
            crate::adapter::TimingDuration::ContextCompaction,
            123,
        )]),
        context_delivery: None,
        compaction: Some(crate::adapter::AdapterCompactionMetrics {
            sequence: 2,
            outcome: crate::adapter::AdapterCompactionOutcome::Completed,
            total_tokens_before: Some(64_000),
            total_tokens_after: Some(4_000),
            model_context_window: Some(128_000),
        }),
    });
    recorder.flush();

    let rows = saved_records(directory.path());
    let row = &rows[0];
    assert_eq!(row["checkpoint"], "adapter_timing");
    assert_eq!(
        row["measurements"]["timing"]["checkpoint"],
        "preparation_compaction_completed"
    );
    assert_eq!(
        row["measurements"]["timing"]["durations_ms"]["context_compaction"],
        123
    );
    assert_eq!(row["measurements"]["timing"]["compaction"]["sequence"], 2);
    assert_eq!(
        row["measurements"]["timing"]["compaction"]["outcome"],
        "completed"
    );
    assert_eq!(
        row["measurements"]["timing"]["compaction"]["total_tokens_before"],
        64_000
    );
    assert!(row["measurements"]["timing"]["compaction"]
        .get("thread_id")
        .is_none());
}

#[test]
fn oversized_history_fallback_is_countable_by_invocation_and_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let recorder = MetricsRecorder::new(&directory.path().join("state.json"));
    let sink = recorder.adapter_sink(context(2), Instant::now());
    sink.record(crate::adapter::AdapterTiming {
        checkpoint: crate::adapter::TimingCheckpoint::OversizedHistoryFork,
        durations_ms: std::collections::BTreeMap::new(),
        context_delivery: None,
        compaction: None,
    });
    recorder.flush();

    let rows = saved_records(directory.path());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["invocation_ref"], context(2).invocation_ref);
    assert_eq!(rows[0]["attempt"], 2);
    assert_eq!(
        rows[0]["measurements"]["timing"]["checkpoint"],
        "oversized_history_fork"
    );
}

#[test]
fn sqlite_metrics_preserve_historic_jsonl_without_new_writes() {
    let directory = tempfile::tempdir().unwrap();
    let metrics = directory.path().join("metrics");
    fs::create_dir(&metrics).unwrap();
    let historic = metrics.join("turns-2026-01-01.jsonl");
    fs::write(&historic, "historic").unwrap();
    let recorder = MetricsRecorder::new(&directory.path().join("state.json"));
    recorder.record(&context(1), "runtime", "attempt_started", json!({}));
    recorder.flush();
    assert_eq!(fs::read_to_string(&historic).unwrap(), "historic");
    assert_eq!(fs::read_dir(metrics).unwrap().count(), 1);
    assert_eq!(saved_records(directory.path()).len(), 1);
}

#[test]
fn final_owner_drop_drains_queued_records_without_explicit_flush() {
    let directory = tempfile::tempdir().unwrap();
    let recorder = MetricsRecorder::new(&directory.path().join("state.json"));
    let last_owner = recorder.clone();
    recorder.record(&context(1), "runtime", "attempt_started", json!({}));
    drop(recorder);
    // Dropping an earlier owner must not shut down the remaining writer.
    last_owner.record(&context(1), "runtime", "attempt_ended", json!({}));
    drop(last_owner);
    let rows = saved_records(directory.path());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["checkpoint"], "attempt_ended");
}

#[test]
fn shutdown_does_not_wait_forever_for_a_stalled_writer() {
    let (sender, _queue) = mpsc::sync_channel(1);
    let (_blocked_writer, finished) = mpsc::channel();
    let writer = MetricsWriter {
        sender: Some(sender),
        finished: Mutex::new(finished),
    };
    let started = Instant::now();
    drop(writer);
    assert!(started.elapsed() >= SHUTDOWN_TIMEOUT);
    assert!(started.elapsed() < SHUTDOWN_TIMEOUT + Duration::from_secs(2));
}
