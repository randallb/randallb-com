use crate::adapter::{
    AdapterCompactionMetrics, AdapterCompactionOutcome, AdapterTiming, TimingCheckpoint,
    TimingDuration,
};
use std::collections::BTreeMap;

struct LiveCompaction {
    item_id: String,
    started_at: Instant,
    sequence: u64,
}

struct TurnTimings {
    provider_started: Instant,
    stage_started: Instant,
    stages: BTreeMap<TimingDuration, u64>,
    turn_sent: Option<Instant>,
    first_activity: bool,
    first_text: bool,
    live_compaction: Option<LiveCompaction>,
    live_compactions: u64,
}

impl TurnTimings {
    fn new(provider_started: Instant) -> Self {
        Self {
            provider_started,
            stage_started: Instant::now(),
            stages: BTreeMap::new(),
            turn_sent: None,
            first_activity: false,
            first_text: false,
            live_compaction: None,
            live_compactions: 0,
        }
    }

    fn stage(&mut self, stage: TimingDuration) {
        let now = Instant::now();
        self.stages.insert(
            stage,
            now.duration_since(self.stage_started).as_millis() as u64,
        );
        self.stage_started = now;
    }

    fn startup_complete(&mut self, handler: &mut impl BoundedActionHandler) {
        if let Some(sent) = self.turn_sent {
            self.stages.insert(
                TimingDuration::ProviderStartup,
                sent.duration_since(self.provider_started).as_millis() as u64,
            );
            handler.on_timing(AdapterTiming {
                checkpoint: TimingCheckpoint::StartupComplete,
                durations_ms: self.stages.clone(),
                context_delivery: None,
                compaction: None,
            });
        }
    }

    fn turn_accepted(&self, received: Instant, handler: &mut impl BoundedActionHandler) {
        if let Some(sent) = self.turn_sent {
            handler.on_timing(AdapterTiming::single(
                TimingCheckpoint::TurnAccepted,
                TimingDuration::TurnAcceptance,
                received.saturating_duration_since(sent).as_millis() as u64,
            ));
        }
    }

    fn activity_received(&mut self, received: Instant, handler: &mut impl BoundedActionHandler) {
        if let Some(sent) = self.turn_sent {
            if !self.first_activity {
                self.first_activity = true;
                handler.on_timing(AdapterTiming::single(
                    TimingCheckpoint::FirstActivity,
                    TimingDuration::FirstActivity,
                    received.saturating_duration_since(sent).as_millis() as u64,
                ));
            }
        }
    }

    fn text_received(&mut self, received: Instant, handler: &mut impl BoundedActionHandler) {
        self.activity_received(received, handler);
        if let Some(sent) = self.turn_sent {
            if !self.first_text {
                self.first_text = true;
                handler.on_timing(AdapterTiming::single(
                    TimingCheckpoint::FirstText,
                    TimingDuration::FirstText,
                    received.saturating_duration_since(sent).as_millis() as u64,
                ));
            }
        }
    }

    fn observe_tool(
        &mut self,
        message: &Value,
        received_at: Instant,
        scope: &CodexOpenResponsesMapper,
        handler: &mut impl BoundedActionHandler,
    ) {
        let thread = message.pointer("/params/threadId").and_then(Value::as_str);
        let turn = message.pointer("/params/turnId").and_then(Value::as_str);
        if thread.is_some_and(|id| id != scope.thread_id())
            || turn.is_some_and(|id| id != scope.turn_id())
        {
            return;
        }
        if message.get("method").and_then(Value::as_str) == Some("item/tool/call")
            && message
                .pointer("/params/tool")
                .and_then(Value::as_str)
                .is_some_and(|tool| !tool.is_empty() && tool != "cwr--return--result")
        {
            self.activity_received(received_at, handler);
        }
        if matches!(
            message.get("method").and_then(Value::as_str),
            Some("item/started" | "item/completed")
        ) && matches!(
            message.pointer("/params/item/type").and_then(Value::as_str),
            Some(
                "commandExecution" | "fileChange" | "webSearch" | "mcpToolCall" | "dynamicToolCall"
            )
        ) {
            self.activity_received(received_at, handler);
        }
    }

    /// Provider notifications are timestamped by the protocol reader. Recording
    /// at this boundary excludes time spent forwarding activities or persisting
    /// conversation state after the notification arrived.
    fn observe_live_compaction(
        &mut self,
        message: &Value,
        received_at: Instant,
        scope: &CodexOpenResponsesMapper,
        handler: &mut impl BoundedActionHandler,
    ) {
        let method = message.get("method").and_then(Value::as_str);
        if !matches!(method, Some("item/started" | "item/completed"))
            || message.pointer("/params/threadId").and_then(Value::as_str)
                != Some(scope.thread_id())
            || message.pointer("/params/turnId").and_then(Value::as_str)
                != Some(scope.turn_id())
            || message.pointer("/params/item/type").and_then(Value::as_str)
                != Some("contextCompaction")
        {
            return;
        }
        let Some(item_id) = message
            .pointer("/params/item/id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
        else {
            return;
        };
        if method == Some("item/started") {
            if self.live_compaction.is_none() {
                self.live_compactions = self.live_compactions.saturating_add(1);
                let compaction = LiveCompaction {
                    item_id: item_id.to_owned(),
                    started_at: received_at,
                    sequence: self.live_compactions,
                };
                handler.on_timing(AdapterTiming {
                    checkpoint: TimingCheckpoint::LiveCompactionStarted,
                    durations_ms: BTreeMap::new(),
                    context_delivery: None,
                    compaction: Some(AdapterCompactionMetrics {
                        sequence: compaction.sequence,
                        outcome: AdapterCompactionOutcome::Running,
                        total_tokens_before: None,
                        total_tokens_after: None,
                        model_context_window: None,
                    }),
                });
                self.live_compaction = Some(compaction);
            }
            return;
        }
        let Some(compaction) = self.live_compaction.take() else {
            return;
        };
        if compaction.item_id != item_id {
            self.live_compaction = Some(compaction);
            return;
        }
        handler.on_timing(AdapterTiming {
            checkpoint: TimingCheckpoint::LiveCompactionCompleted,
            durations_ms: BTreeMap::from([(
                TimingDuration::ContextCompaction,
                received_at
                    .saturating_duration_since(compaction.started_at)
                    .as_millis() as u64,
            )]),
            context_delivery: None,
            compaction: Some(AdapterCompactionMetrics {
                sequence: compaction.sequence,
                outcome: AdapterCompactionOutcome::Completed,
                total_tokens_before: None,
                total_tokens_after: None,
                model_context_window: None,
            }),
        });
    }

    fn fail_live_compaction(
        &mut self,
        received_at: Instant,
        handler: &mut impl BoundedActionHandler,
    ) {
        let Some(compaction) = self.live_compaction.take() else {
            return;
        };
        handler.on_timing(AdapterTiming {
            checkpoint: TimingCheckpoint::LiveCompactionFailed,
            durations_ms: BTreeMap::from([(
                TimingDuration::ContextCompaction,
                received_at
                    .saturating_duration_since(compaction.started_at)
                    .as_millis() as u64,
            )]),
            context_delivery: None,
            compaction: Some(AdapterCompactionMetrics {
                sequence: compaction.sequence,
                outcome: AdapterCompactionOutcome::Failed,
                total_tokens_before: None,
                total_tokens_after: None,
                model_context_window: None,
            }),
        });
    }
}

// Startup activity callbacks synchronously wait for the runtime. Separate this
// time from context delivery, including every history-checkpoint acknowledgement.
struct StartupTimingHandler<'a, H> {
    inner: &'a mut H,
    acknowledgement: Duration,
    startup_complete: bool,
}

impl<'a, H> StartupTimingHandler<'a, H> {
    fn new(inner: &'a mut H) -> Self {
        Self {
            inner,
            acknowledgement: Duration::ZERO,
            startup_complete: false,
        }
    }
}

impl<H: BoundedActionHandler> BoundedActionHandler for StartupTimingHandler<'_, H> {
    fn on_timing(&mut self, mut timing: AdapterTiming) {
        if matches!(
            timing.checkpoint,
            TimingCheckpoint::StartupComplete | TimingCheckpoint::ProviderEnded
        ) {
            let acknowledgement_ms = self.acknowledgement.as_millis() as u64;
            if let Some(context_ms) = timing
                .durations_ms
                .get_mut(&TimingDuration::ContextInjection)
            {
                *context_ms = context_ms.saturating_sub(acknowledgement_ms);
            }
            timing
                .durations_ms
                .insert(TimingDuration::HostAcknowledgement, acknowledgement_ms);
        }
        if timing.checkpoint == TimingCheckpoint::StartupComplete {
            self.startup_complete = true;
        }
        self.inner.on_timing(timing);
    }

    fn is_stopped(&self) -> bool {
        self.inner.is_stopped()
    }
    fn claim_terminal(&mut self) -> bool {
        self.inner.claim_terminal()
    }
    fn on_progress(&mut self, status: u16, summary: &str) -> CwrResult<()> {
        self.inner.on_progress(status, summary)
    }
    fn on_public_progress(&mut self, status: u16, summary: &str) -> CwrResult<()> {
        self.inner.on_public_progress(status, summary)
    }
    fn on_activity(&mut self, activity: AdapterActivityEvent) -> CwrResult<()> {
        let started = Instant::now();
        let result = self.inner.on_activity(activity);
        if !self.startup_complete {
            self.acknowledgement += started.elapsed();
        }
        result
    }
    fn on_model_output(&mut self, output: AdapterModelOutput) -> CwrResult<()> {
        self.inner.on_model_output(output)
    }
    fn on_host_call(
        &mut self,
        call_id: &str,
        operation: &str,
        input: Value,
    ) -> CwrResult<ToolCallResult> {
        self.inner.on_host_call(call_id, operation, input)
    }
}
