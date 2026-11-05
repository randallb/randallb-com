impl LocalRuntime {
    fn start_attempt_metrics(
        &self,
        record: &ActionInvocationRecord,
        invocation: &AdapterInvocation,
        claimed_at: u64,
        attempt_started: std::time::Instant,
        claim_persistence_ms: u64,
    ) -> crate::metrics::AttemptMetrics {
        let metadata_started = std::time::Instant::now();
        let metadata = &invocation.adapter_params;
        let short_string = |key: &str| {
            metadata
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| value.len() <= 128)
                .map(str::to_owned)
        };
        let context = crate::metrics::MetricContext {
            runtime_ref: self
                .lock()
                .map(|state| state.admission.runtime_root_ref().to_owned())
                .unwrap_or_default(),
            invocation_ref: record.invocation_ref.clone(),
            conversation_ref: record.conversation_ref.clone(),
            parent_invocation_ref: record.parent_invocation_ref.clone(),
            attempt: record.attempt_count,
            model_run_ref: invocation.model_run_ref.clone(),
            actor_ref: record.target_actor_ref.clone(),
            model: short_string("model"),
            reasoning_effort: short_string("reasoning_effort"),
            invocation_continuation_supplied: invocation.continuation.is_some(),
            private_history_supplied: private_history_supplied(&invocation.thread),
            context_bytes: encoded_context_bytes(&invocation.thread),
        };
        let metrics = crate::metrics::AttemptMetrics {
            recorder: self.metrics.clone(),
            context,
            started: attempt_started,
            outcome: "failed".into(),
            total_turn_ms: None,
        };
        metrics.recorder.record(&metrics.context, "runtime", "attempt_started", json!({
            "queue_ms": (record.attempt_count == 1).then(|| claimed_at.checked_sub(record.admitted_at_unix_ms)).flatten(),
            "claim_persistence_ms": claim_persistence_ms,
            "metrics_metadata_ms": crate::metrics::elapsed_ms(metadata_started),
        }));
        metrics
    }

    fn finish_attempt_metrics(&self, metrics: &mut crate::metrics::AttemptMetrics) {
        if let Ok(state) = self.lock() {
            if let Some(current) = state
                .admission
                .invocation_record(&metrics.context.invocation_ref)
            {
                if current.state.is_terminal() {
                    metrics.outcome = serde_json::to_value(current.state)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "unknown".into());
                    metrics.total_turn_ms = current
                        .finished_at_unix_ms
                        .and_then(|end| end.checked_sub(current.admitted_at_unix_ms));
                }
            }
        }
    }
}

fn private_history_supplied(thread: &Value) -> bool {
    crate::adapter::working_context(thread)
        .and_then(|context| context.pointer("/working_conversation/state"))
        .is_some_and(|state| !state.is_null())
}

#[cfg(test)]
mod history_metrics_tests {
    use super::*;

    #[test]
    fn private_history_metric_follows_the_supplied_working_state() {
        assert!(!private_history_supplied(&json!({
            "working_conversation": {"ref": "new-thread", "history_gap": "earlier work failed"}
        })));
        assert!(!private_history_supplied(&json!({
            "working_conversation": {"ref": "new-thread", "state": null}
        })));
        assert!(private_history_supplied(&json!({
            "working_conversation": {"ref": "prior-thread", "state": {"idle": true}}
        })));
        assert!(private_history_supplied(&json!({
            "requesting_context": {
                "working_conversation": {"ref": "prior-thread", "state": {"idle": true}}
            }
        })));
    }
}

fn encoded_context_bytes(context: &Value) -> usize {
    struct ByteCounter(usize);
    impl std::io::Write for ByteCounter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, context).map_or(0, |()| counter.0)
}
