//! Invocation lifetime measurements. No invocation payload crosses this boundary.
use super::*;
use crate::v2026_09::invocation::{ActionInvocationRecord, ActionInvocationState};
use std::collections::BTreeMap;

const MAX_TRACKED: usize = 4096;

pub(super) fn configured_threshold() -> Duration {
    Duration::from_secs(
        std::env::var("CWR_SLOW_INVOCATION_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| (1..=86_400).contains(value))
            .unwrap_or(30),
    )
}

pub(super) struct Sample {
    context: MetricContext,
    conversation_ref: Option<String>,
    source_message_ref: Option<String>,
    state: ActionInvocationState,
    pub(super) observed: Instant,
    recovered_age: Option<Duration>,
    started: bool,
    attempt_active: bool,
}

impl Sample {
    fn settled(&self) -> bool {
        self.state.is_terminal() && !self.attempt_active
    }

    pub(super) fn new(
        runtime_ref: &str,
        record: &ActionInvocationRecord,
        source_message_ref: Option<&str>,
        recovered: bool,
    ) -> Self {
        Self {
            context: MetricContext {
                runtime_ref: runtime_ref.into(),
                invocation_ref: record.invocation_ref.clone(),
                conversation_ref: record.conversation_ref.clone(),
                parent_invocation_ref: record.parent_invocation_ref.clone(),
                attempt: record.attempt_count,
                model_run_ref: None,
                actor_ref: record.target_actor_ref.clone(),
                model: None,
                reasoning_effort: None,
                invocation_continuation_supplied: false,
                private_history_supplied: false,
                context_bytes: 0,
            },
            conversation_ref: record.conversation_ref.clone(),
            source_message_ref: source_message_ref.map(str::to_owned),
            state: record.state,
            observed: Instant::now(),
            recovered_age: recovered.then(|| {
                Duration::from_millis(unix_ms().saturating_sub(record.admitted_at_unix_ms))
            }),
            started: record.started_at_unix_ms.is_some(),
            attempt_active: record.attempt_active,
        }
    }
}

struct Tracked {
    sample: Sample,
    admitted: Instant,
    prior_age: Duration,
    approximate: bool,
    queue_ms: Option<u64>,
    reported: bool,
}

impl Tracked {
    fn elapsed(&self, now: Instant) -> Duration {
        self.prior_age
            .saturating_add(now.saturating_duration_since(self.admitted))
    }

    fn record(&self, checkpoint: &'static str, now: Instant, threshold: Duration) -> MetricRecord {
        MetricRecord {
            schema_version: 2,
            recorded_at_unix_ms: unix_ms(),
            process_id: std::process::id(),
            app_version: env!("CARGO_PKG_VERSION"),
            context: self.sample.context.clone(),
            producer: "runtime",
            checkpoint,
            measurements: json!({
                "conversation_ref": self.sample.conversation_ref,
                "source_message_ref": self.sample.source_message_ref,
                "threshold_ms": threshold.as_millis() as u64,
                "elapsed_ms": self.elapsed(now).as_millis() as u64,
                "elapsed_is_approximate": self.approximate,
                "queue_ms": self.queue_ms,
                "state": self.sample.state,
                "outcome": self.sample.settled().then_some(self.sample.state),
            }),
            dropped_records_total: 0,
        }
    }
}

pub(super) struct Monitor {
    threshold: Duration,
    tracked: BTreeMap<String, Tracked>,
}

impl Monitor {
    pub(super) fn new(threshold: Duration) -> Self {
        Self {
            threshold,
            tracked: BTreeMap::new(),
        }
    }

    pub(super) fn metadata(&mut self, record: &MetricRecord) {
        if record.checkpoint == "attempt_started" {
            if let Some(tracked) = self.tracked.get_mut(&record.context.invocation_ref) {
                tracked.sample.context = record.context.clone();
            }
        }
    }

    pub(super) fn update(
        &mut self,
        mut sample: Sample,
        records: &mut Vec<MetricRecord>,
        dropped: &AtomicU64,
    ) {
        let key = sample.context.invocation_ref.clone();
        if let Some(tracked) = self.tracked.get_mut(&key) {
            if sample.source_message_ref.is_none() {
                sample.source_message_ref = tracked.sample.source_message_ref.take();
            }
            if sample.started && !tracked.sample.started && !tracked.approximate {
                tracked.queue_ms = Some(
                    sample
                        .observed
                        .saturating_duration_since(tracked.admitted)
                        .as_millis() as u64,
                );
            }
            sample.context.model = tracked.sample.context.model.clone();
            sample.context.reasoning_effort = tracked.sample.context.reasoning_effort.clone();
            sample.context.model_run_ref = tracked.sample.context.model_run_ref.clone();
            sample.context.conversation_ref = tracked.sample.context.conversation_ref.clone();
            sample.context.invocation_continuation_supplied =
                tracked.sample.context.invocation_continuation_supplied;
            sample.context.private_history_supplied =
                tracked.sample.context.private_history_supplied;
            sample.context.context_bytes = tracked.sample.context.context_bytes;
            tracked.sample = sample;
        } else {
            // A lost admission sample must not turn a terminal observation into a
            // fabricated duration. Existing process metrics still record that outcome.
            if sample.settled() {
                return;
            }
            if self.tracked.len() == MAX_TRACKED {
                note_drop(dropped);
                return;
            }
            self.tracked.insert(
                key.clone(),
                Tracked {
                    admitted: sample.observed,
                    prior_age: sample.recovered_age.unwrap_or_default(),
                    approximate: sample.recovered_age.is_some() || sample.started,
                    queue_ms: None,
                    reported: false,
                    sample,
                },
            );
        }
        let tracked = self.tracked.get_mut(&key).expect("sample was inserted");
        if tracked.sample.settled() {
            let now = tracked.sample.observed;
            if !tracked.reported && tracked.elapsed(now) >= self.threshold {
                records.push(tracked.record("slow_invocation_threshold", now, self.threshold));
                tracked.reported = true;
            }
            if tracked.reported {
                records.push(tracked.record("slow_invocation_ended", now, self.threshold));
            }
            self.tracked.remove(&key);
        }
    }

    pub(super) fn tick(&mut self, now: Instant, records: &mut Vec<MetricRecord>) {
        for tracked in self.tracked.values_mut() {
            if !tracked.reported && tracked.elapsed(now) >= self.threshold {
                records.push(tracked.record("slow_invocation_threshold", now, self.threshold));
                tracked.reported = true;
            }
        }
    }
}

#[cfg(test)]
mod tests;
