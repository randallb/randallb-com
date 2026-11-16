use super::*;
use crate::adapter::{
    AdapterCompactionMetrics, AdapterCompactionOutcome, AdapterTiming, TimingCheckpoint,
    TimingDuration,
};

impl HistoryPreparation {
    pub(super) fn should_compact_before(&self, unit: &HistoryUnit) -> bool {
        if self.checkpoint.pending || !self.checkpoint.compaction.is_idle() {
            return false;
        }
        let unknown_prior_occupancy = self.prior_thread.is_some()
            && self.checkpoint.latest_usage.is_none()
            && self.checkpoint.compactions == 0;
        if unknown_prior_occupancy && self.checkpoint.confirmed_items == 0 {
            return true;
        }
        self.checkpoint
            .estimated_input_tokens
            .saturating_add(unit.estimated_tokens)
            > self.compaction_threshold()
    }

    pub(super) fn compaction_threshold(&self) -> usize {
        self.checkpoint
            .model_context_window
            .map(|window| {
                let proportional_reserve = window / 4;
                let reserve = CONTEXT_RESERVE_TOKENS.min(proportional_reserve);
                (window.saturating_mul(3) / 4).min(window.saturating_sub(reserve))
            })
            .filter(|threshold| *threshold > 0)
            .unwrap_or(UNKNOWN_MODEL_COMPACTION_THRESHOLD)
    }

    pub(super) fn compaction_request_accepted(
        &mut self,
        received_at: Instant,
        handler: &mut impl BoundedActionHandler,
    ) -> CwrResult<bool> {
        match &mut self.checkpoint.compaction {
            CompactionState::Requested { request_accepted }
            | CompactionState::Running {
                request_accepted, ..
            } => *request_accepted = true,
            CompactionState::Idle => {
                return invalid("Codex acknowledged an unexpected context compaction")
            }
        }
        self.persist(handler)?;
        self.finish_compaction_if_complete(received_at, handler)
    }

    /// Returns true when both the native compaction item and its successful
    /// turn have completed and history injection may resume.
    pub(super) fn observe_compaction(
        &mut self,
        message: &Value,
        received_at: Instant,
        handler: &mut impl BoundedActionHandler,
    ) -> CwrResult<bool> {
        if self.checkpoint.compaction.is_idle() {
            return Ok(false);
        }
        let method = message.get("method").and_then(Value::as_str);
        let message_thread = message.pointer("/params/threadId").and_then(Value::as_str);
        if message_thread.is_some_and(|thread| thread != self.checkpoint.thread_id) {
            return Ok(false);
        }
        if method == Some("turn/started") {
            let turn_id = message
                .pointer("/params/turn/id")
                .and_then(Value::as_str)
                .and_then(nonblank)
                .ok_or("Codex compaction turn has no turn ID")?;
            let request_accepted = match &self.checkpoint.compaction {
                CompactionState::Requested { request_accepted } => *request_accepted,
                CompactionState::Running {
                    turn_id: active_turn,
                    ..
                } if active_turn == turn_id => return Ok(false),
                CompactionState::Running { .. } | CompactionState::Idle => return Ok(false),
            };
            self.checkpoint.compaction = CompactionState::Running {
                turn_id: turn_id.to_owned(),
                request_accepted,
                item_completed: false,
                turn_completed: false,
            };
            self.persist(handler)?;
            report_execution_phase(
                handler,
                crate::adapter::ExecutionPhase::CompactingContext,
                crate::adapter::ExecutionPhaseState::Started,
            )?;
            return Ok(false);
        }
        let CompactionState::Running { turn_id, .. } = &self.checkpoint.compaction else {
            return Ok(false);
        };
        let event_turn = message
            .pointer("/params/turnId")
            .and_then(Value::as_str)
            .or_else(|| message.pointer("/params/turn/id").and_then(Value::as_str));
        if event_turn != Some(turn_id.as_str()) {
            return Ok(false);
        }
        if method == Some("thread/tokenUsage/updated") {
            self.observe_token_usage(message);
            return Ok(false);
        }
        if method == Some("item/completed")
            && message
                .pointer("/params/item/type")
                .and_then(Value::as_str)
                == Some("contextCompaction")
        {
            let CompactionState::Running { item_completed, .. } =
                &mut self.checkpoint.compaction
            else {
                return Ok(false);
            };
            *item_completed = true;
            self.persist(handler)?;
            return self.finish_compaction_if_complete(received_at, handler);
        }
        if method != Some("turn/completed") {
            return Ok(false);
        }
        let status = message
            .pointer("/params/turn/status")
            .and_then(Value::as_str)
            .unwrap_or("completed");
        if status != "completed" {
            self.fail_compaction(received_at, handler);
            return invalid(format!(
                "Codex context compaction {status}: {}",
                message.pointer("/params/turn/error").unwrap_or(&Value::Null)
            ));
        }
        let CompactionState::Running { turn_completed, .. } =
            &mut self.checkpoint.compaction
        else {
            return Ok(false);
        };
        *turn_completed = true;
        self.persist(handler)?;
        self.finish_compaction_if_complete(received_at, handler)
    }

    fn finish_compaction_if_complete(
        &mut self,
        completed_at: Instant,
        handler: &mut impl BoundedActionHandler,
    ) -> CwrResult<bool> {
        let CompactionState::Running {
            request_accepted,
            item_completed,
            turn_completed,
            ..
        } = &self.checkpoint.compaction
        else {
            return Ok(false);
        };
        if !request_accepted || !item_completed || !turn_completed {
            return Ok(false);
        }
        let fallback = self.compaction_threshold() / 4;
        let completed_usage = self.checkpoint.latest_usage.clone();
        let compacted_tokens = self
            .checkpoint
            .latest_usage
            .as_ref()
            .map_or(fallback, |usage| usage.total_tokens);
        self.checkpoint.estimated_input_tokens =
            compacted_tokens.saturating_add(self.live_input_tokens);
        self.checkpoint.compaction = CompactionState::Idle;
        self.checkpoint.confirmed_at_last_compaction = self.checkpoint.confirmed_items;
        self.checkpoint.compactions += 1;
        let sequence = u64::try_from(self.checkpoint.compactions).unwrap_or(u64::MAX);
        if let Some(started_at) = self.compaction_started_at.take() {
            let started_usage = self.compaction_start_usage.take();
            // Both endpoints are captured by the provider reader. This excludes
            // checkpoint persistence and activity acknowledgement after the
            // provider reported completion.
            handler.on_timing(AdapterTiming {
                checkpoint: TimingCheckpoint::PreparationCompactionCompleted,
                durations_ms: std::collections::BTreeMap::from([(
                    TimingDuration::ContextCompaction,
                    completed_at
                        .saturating_duration_since(started_at)
                        .as_millis() as u64,
                )]),
                context_delivery: None,
                compaction: Some(AdapterCompactionMetrics {
                    sequence,
                    outcome: AdapterCompactionOutcome::Completed,
                    total_tokens_before: started_usage
                        .as_ref()
                        .and_then(|usage| u64::try_from(usage.total_tokens).ok()),
                    total_tokens_after: completed_usage
                        .as_ref()
                        .and_then(|usage| u64::try_from(usage.total_tokens).ok()),
                    model_context_window: completed_usage
                        .as_ref()
                        .and_then(|usage| usage.model_context_window)
                        .or_else(|| started_usage.and_then(|usage| usage.model_context_window))
                        .and_then(|tokens| u64::try_from(tokens).ok()),
                }),
            });
        }
        self.persist(handler)?;
        report_execution_phase(
            handler,
            crate::adapter::ExecutionPhase::CompactingContext,
            crate::adapter::ExecutionPhaseState::Completed,
        )?;
        self.restoration_needs_refresh = true;
        Ok(true)
    }

    pub(super) fn fail_compaction(
        &mut self,
        received_at: Instant,
        handler: &mut impl BoundedActionHandler,
    ) {
        let Some(started_at) = self.compaction_started_at.take() else {
            return;
        };
        let started_usage = self.compaction_start_usage.take();
        let sequence = u64::try_from(self.checkpoint.compactions.saturating_add(1))
            .unwrap_or(u64::MAX);
        handler.on_timing(AdapterTiming {
            checkpoint: TimingCheckpoint::PreparationCompactionFailed,
            durations_ms: std::collections::BTreeMap::from([(
                TimingDuration::ContextCompaction,
                received_at
                    .saturating_duration_since(started_at)
                    .as_millis() as u64,
            )]),
            context_delivery: None,
            compaction: Some(AdapterCompactionMetrics {
                sequence,
                outcome: AdapterCompactionOutcome::Failed,
                total_tokens_before: started_usage
                    .as_ref()
                    .and_then(|usage| u64::try_from(usage.total_tokens).ok()),
                total_tokens_after: self
                    .checkpoint
                    .latest_usage
                    .as_ref()
                    .and_then(|usage| u64::try_from(usage.total_tokens).ok()),
                model_context_window: self
                    .checkpoint
                    .latest_usage
                    .as_ref()
                    .and_then(|usage| usage.model_context_window)
                    .or_else(|| started_usage.and_then(|usage| usage.model_context_window))
                    .and_then(|tokens| u64::try_from(tokens).ok()),
            }),
        });
    }
}

fn report_execution_phase(
    handler: &mut impl BoundedActionHandler,
    phase: crate::adapter::ExecutionPhase,
    state: crate::adapter::ExecutionPhaseState,
) -> CwrResult<()> {
    handler.on_activity(AdapterActivityEvent::ExecutionActivity {
        activity: crate::adapter::ExecutionActivity {
            phase,
            state,
            completed_parts: None,
            total_parts: None,
        },
    })
}
