//! Best-effort local performance records, independent of conversation persistence.
use crate::adapter::{AdapterTiming, AdapterTimingSink};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod posthog;
mod slow;
mod sqlite;

const QUEUE_CAPACITY: usize = 256;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

pub use posthog::{
    environment_fingerprint as posthog_environment_fingerprint,
    flush_pending as flush_posthog_metrics,
};

/// Filename for CWR's disposable local performance database beside runtime state.
pub const LOCAL_METRICS_DATABASE_FILENAME: &str = "metrics.sqlite3";

#[derive(Clone, Serialize)]
pub(crate) struct MetricContext {
    pub runtime_ref: String,
    pub invocation_ref: String,
    /// Written as record data and, when present, indexed through a disposable
    /// metrics relationship. Retention never removes a durable application
    /// binding for this reference.
    pub conversation_ref: Option<String>,
    pub parent_invocation_ref: Option<String>,
    pub attempt: u64,
    pub model_run_ref: Option<String>,
    pub actor_ref: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    /// A checkpoint for this same invocation was supplied after an interrupted attempt.
    pub invocation_continuation_supplied: bool,
    /// CWR supplied settled private history from an earlier invocation.
    /// The adapter may still fail before the provider accepts its fork.
    pub private_history_supplied: bool,
    pub context_bytes: usize,
}

#[derive(Clone, Serialize)]
struct MetricRecord {
    schema_version: u8,
    recorded_at_unix_ms: u64,
    process_id: u32,
    app_version: &'static str,
    #[serde(flatten)]
    context: MetricContext,
    producer: &'static str,
    checkpoint: &'static str,
    measurements: Value,
    dropped_records_total: u64,
}

enum WriterMessage {
    Record(Box<MetricRecord>, Instant),
    Invocation(Box<slow::Sample>),
    #[cfg(test)]
    Flush(mpsc::Sender<()>),
}

#[derive(Clone)]
pub(crate) struct MetricsRecorder {
    writer: Arc<MetricsWriter>,
    dropped: Arc<AtomicU64>,
}

// One owner shared by runtime clones and in-flight attempts. Only the final
// drop closes the queue; ordinary record/sink drops never wait for storage.
struct MetricsWriter {
    sender: Option<mpsc::SyncSender<WriterMessage>>,
    finished: Mutex<mpsc::Receiver<()>>,
}

impl Drop for MetricsWriter {
    fn drop(&mut self) {
        drop(self.sender.take());
        let finished = self.finished.get_mut().unwrap_or_else(|e| e.into_inner());
        if matches!(
            finished.recv_timeout(SHUTDOWN_TIMEOUT),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            eprintln!("cwr: metrics shutdown timed out; queued records may be lost");
        }
    }
}

impl MetricsRecorder {
    pub fn new(state_path: &Path) -> Self {
        Self::with_slow_threshold(state_path, slow::configured_threshold())
    }

    pub(crate) fn with_slow_threshold(state_path: &Path, threshold: Duration) -> Self {
        let metrics_database_path = state_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(LOCAL_METRICS_DATABASE_FILENAME);
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (finished_send, finished_receive) = mpsc::channel();
        let dropped = Arc::new(AtomicU64::new(0));
        let failures = dropped.clone();
        let spawn = std::thread::Builder::new()
            .name("cwr-metrics".into())
            .spawn(move || {
                run_writer(&metrics_database_path, receiver, &failures, threshold);
                let _ = finished_send.send(());
            });
        if spawn.is_err() {
            note_drop(&dropped);
        }
        Self {
            writer: Arc::new(MetricsWriter {
                sender: Some(sender),
                finished: Mutex::new(finished_receive),
            }),
            dropped,
        }
    }

    pub fn record(
        &self,
        context: &MetricContext,
        producer: &'static str,
        checkpoint: &'static str,
        measurements: Value,
    ) {
        let record = MetricRecord {
            schema_version: 2,
            recorded_at_unix_ms: unix_ms(),
            process_id: std::process::id(),
            app_version: env!("CARGO_PKG_VERSION"),
            context: context.clone(),
            producer,
            checkpoint,
            measurements,
            dropped_records_total: 0,
        };
        if self
            .writer
            .sender
            .as_ref()
            .expect("metrics writer is open while owned")
            .try_send(WriterMessage::Record(Box::new(record), Instant::now()))
            .is_err()
        {
            note_drop(&self.dropped);
        }
    }

    pub(crate) fn observe_invocation(
        &self,
        runtime_ref: &str,
        record: &crate::v2026_09::invocation::ActionInvocationRecord,
        source_message_ref: Option<&str>,
        recovered: bool,
    ) {
        let sample = slow::Sample::new(runtime_ref, record, source_message_ref, recovered);
        if self
            .writer
            .sender
            .as_ref()
            .expect("metrics writer is open while owned")
            .try_send(WriterMessage::Invocation(Box::new(sample)))
            .is_err()
        {
            note_drop(&self.dropped);
        }
    }

    pub(crate) fn record_adapter_process_failure(
        &self,
        context: &MetricContext,
        error: &(dyn std::error::Error + Send + Sync + 'static),
    ) {
        let Some(failure) = error.downcast_ref::<crate::adapter::ProcessAdapterFailure>() else {
            return;
        };
        self.record(
            context,
            "process_transport",
            "adapter_process_failed",
            json!({
                "phase": "awaiting_result",
                "exit_code": failure.exit_code,
                "signal": failure.signal,
                "stdout_state": failure.stdout_state,
                "stderr_total_bytes": failure.stderr_total_bytes,
                "stderr_truncated": failure.stderr_truncated,
            }),
        );
    }

    pub fn adapter_sink(&self, context: MetricContext, claimed: Instant) -> AdapterTimingSink {
        let recorder = self.clone();
        AdapterTimingSink::new(move |timing: AdapterTiming| {
            let producer = if matches!(
                timing.checkpoint,
                crate::adapter::TimingCheckpoint::AdapterSpawned
                    | crate::adapter::TimingCheckpoint::ContextDelivered
            ) {
                "process_transport"
            } else {
                "codex_adapter"
            };
            recorder.record(
                &context,
                producer,
                "adapter_timing",
                json!({
                    "timing": timing, "attempt_elapsed_at_receive_ms": elapsed_ms(claimed),
                    "provider_process_reused": (producer == "codex_adapter").then_some(false)
                }),
            );
        })
    }

    #[cfg(test)]
    pub(crate) fn flush(&self) {
        let (send, receive) = mpsc::channel();
        self.writer
            .sender
            .as_ref()
            .unwrap()
            .send(WriterMessage::Flush(send))
            .unwrap();
        receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }
}

fn run_writer(
    metrics_database_path: &Path,
    receiver: mpsc::Receiver<WriterMessage>,
    failures: &AtomicU64,
    threshold: Duration,
) {
    let exporter = posthog::Exporter::from_environment(metrics_database_path);
    let mut reported_drops = 0;
    let mut slow = slow::Monitor::new(threshold);
    // The writer owns this connection, so collection remains off every
    // invocation's execution path. It is deliberately separate from CWR's
    // authoritative runtime database: losing it cannot change a turn.
    let mut performance = sqlite::MetricStore::new(metrics_database_path);
    let mut next_tick = None;
    loop {
        let mut message = receiver.recv_timeout(Duration::from_millis(250));
        let mut records = Vec::new();
        #[cfg(test)]
        let mut flushes = Vec::new();
        // Consume queued lifecycle updates before evaluating current age.
        // Storage backlog must not turn already-finished work into slow work.
        let mut drained = false;
        let mut disconnected = false;
        let mut observed = None;
        for index in 0..QUEUE_CAPACITY {
            match message {
                Ok(WriterMessage::Record(record, at)) => {
                    observed = Some(at);
                    slow.metadata(&record);
                    records.push(*record);
                }
                Ok(WriterMessage::Invocation(sample)) => {
                    observed = Some(sample.observed);
                    slow.update(*sample, &mut records, failures);
                }
                #[cfg(test)]
                Ok(WriterMessage::Flush(done)) => {
                    flushes.push(done);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    drained = true;
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
            if index + 1 == QUEUE_CAPACITY {
                break;
            }
            message = match receiver.try_recv() {
                Ok(next) => Ok(next),
                Err(mpsc::TryRecvError::Empty) => {
                    drained = true;
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            };
        }
        // When input stays busy, advance only to the processed event's time;
        // later queued completions must not be compared with the current clock.
        let tick_at = if drained {
            Some(Instant::now())
        } else {
            observed
        };
        if let Some(now) = tick_at {
            if next_tick.is_none_or(|next| now >= next) {
                slow.tick(now, &mut records);
                next_tick = Some(now + Duration::from_millis(250));
            }
        }
        for record in &mut records {
            record.dropped_records_total = failures.load(Ordering::Relaxed);
        }
        if let Some(exporter) = &exporter {
            exporter.record(&records);
        }
        match performance.write(&records) {
            Ok(evicted) => note_drops(failures, evicted),
            Err(error) => {
                note_drops(failures, error.unwritten.saturating_add(error.evicted));
                eprintln!("cwr: SQLite metrics write failed: {}", error.error);
            }
        }
        #[cfg(test)]
        for done in flushes {
            let _ = done.send(());
        }
        if disconnected {
            break;
        }
        let count = failures.load(Ordering::Relaxed);
        if count != reported_drops {
            eprintln!("cwr: metrics records dropped: {count}");
            reported_drops = count;
        }
    }
}

pub(crate) fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn note_drop(dropped: &AtomicU64) {
    note_drops(dropped, 1);
}
fn note_drops(dropped: &AtomicU64, count: u64) {
    if count > 0 {
        dropped.fetch_add(count, Ordering::Relaxed);
    }
}

/// A start record plus a guaranteed best-effort end on every normal unwind path.
pub(crate) struct AttemptMetrics {
    pub recorder: MetricsRecorder,
    pub context: MetricContext,
    pub started: Instant,
    pub outcome: String,
    pub total_turn_ms: Option<u64>,
}

impl AttemptMetrics {
    /// Record setup work even when it returns an error. No request content is retained.
    pub(crate) fn measure_setup<T>(&self, stage: &'static str, work: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let result = work();
        self.recorder.record(
            &self.context,
            "runtime",
            "runtime_setup",
            json!({
                "stage": stage,
                "duration_ms": elapsed_ms(started),
                "attempt_elapsed_ms": elapsed_ms(self.started),
            }),
        );
        result
    }
}

impl Drop for AttemptMetrics {
    fn drop(&mut self) {
        self.recorder.record(
            &self.context,
            "runtime",
            "attempt_ended",
            json!({
                "attempt_elapsed_ms": elapsed_ms(self.started), "outcome": self.outcome,
                "total_turn_ms": self.total_turn_ms,
            }),
        );
    }
}

#[cfg(test)]
mod tests;
