// Provider preparation is separate from live execution. No user tool may run here.
use serde::{Deserialize, Serialize};

#[path = "history_digest.rs"]
mod history_digest;
use history_digest::context_digest;
#[path = "history_compaction.rs"]
mod history_compaction;
#[path = "history_source.rs"]
mod history_source;
use history_source::{
    append_context_source, append_history_units, attributed_history_text, history_items,
    model_coworker_directory_digest, read_history_unit,
};
#[cfg(test)]
use history_source::model_source_metadata;

// This bounds transport fragments only. Codex owns its model context budget.
const HISTORY_PART_BYTES: usize = 32 * 1024;
const UNKNOWN_MODEL_COMPACTION_THRESHOLD: usize = 96 * 1024;
const CONTEXT_RESERVE_TOKENS: usize = 32 * 1024;
// Version 5 delivers bounded follow-up source records with the live turn and
// stages oversized records on a fork. Older checkpoints injected every source
// record during preparation and must rebuild from their pinned sources.
const PREPARATION_CHECKPOINT_VERSION: u8 = 5;

#[derive(Clone)]
struct HistoryUnit {
    source_id: String,
    offset: u64,
    bytes: usize,
    part: usize,
    parts: usize,
    estimated_tokens: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeTokenUsage {
    thread_id: String,
    turn_id: String,
    total_tokens: usize,
    model_context_window: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum CompactionState {
    #[default]
    Idle,
    Requested {
        request_accepted: bool,
    },
    Running {
        turn_id: String,
        request_accepted: bool,
        item_completed: bool,
        turn_completed: bool,
    },
}

impl CompactionState {
    fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum HistoryDelivery {
    #[default]
    Injected,
    LiveTurn,
    OversizedFork,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextCheckpoint {
    version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_store: Option<String>,
    invocation_ref: Value,
    integration_ref: Value,
    source_digest: String,
    initialization_digest: String,
    context_snapshot: Value,
    thread_id: String,
    confirmed_items: usize,
    pending: bool,
    #[serde(default)]
    compaction: CompactionState,
    #[serde(default)]
    confirmed_at_last_compaction: usize,
    #[serde(default)]
    compactions: usize,
    #[serde(default)]
    estimated_input_tokens: usize,
    #[serde(default)]
    estimator: String,
    #[serde(default)]
    latest_usage: Option<NativeTokenUsage>,
    #[serde(default)]
    model_context_window: Option<usize>,
    #[serde(default)]
    usage_stale: bool,
    /// Whether source records are injected during preparation, carried by the
    /// live turn, or staged on an oversized private fork.
    #[serde(default)]
    source_delivery: HistoryDelivery,
    live_started: bool,
}

struct HistoryPreparation {
    prior_thread: Option<String>,
    working_ref: Option<String>,
    spool: tempfile::NamedTempFile,
    units: Vec<HistoryUnit>,
    live_input_tokens: usize,
    checkpoint: ContextCheckpoint,
    last_reported_items: Option<usize>,
    restoration_needs_refresh: bool,
    coworker_directory_digest: Option<String>,
    // These are intentionally process-local. A resumed preparation has no
    // trustworthy monotonic origin for a partial compaction, so it records no
    // span instead of fabricating one.
    compaction_started_at: Option<Instant>,
    compaction_start_usage: Option<NativeTokenUsage>,
}

impl HistoryPreparation {
    fn new(config: &BoundedActionConfig) -> CwrResult<Self> {
        let observation = &config.observation;
        let action = &observation["action_request"];
        let source_digest = context_digest(observation)?;
        let coworker_directory_digest = observation
            .get("coworkers")
            .map(model_coworker_directory_digest)
            .transpose()?;
        let mut initialization = json!({
            "instructions":config.developer_instructions,"return_schema":config.return_schema,
            "model":config.model,"reasoning_effort":config.reasoning_effort,
            "workspace":config.workspace,
            "native_text_result":config.native_text_result,"sandbox":config.sandbox,
            "provider_access":match config.provider_access { ProviderAccess::Normal => "normal", ProviderAccess::Isolated => "isolated", ProviderAccess::CwrOnly => "cwr_only" },
            "tools":config.host_tools.iter().map(|tool| json!({"namespace":tool.namespace,"name":tool.name,"operation":tool.operation,
                "description":tool.description,"schema":tool.input_schema})).collect::<Vec<_>>()
        });
        let initialization_digest = context_digest(&initialization)?;
        // Existing conversations used this field in their compatibility digest.
        // It no longer controls input admission or provider configuration.
        initialization["context_window_tokens"] = json!(config.legacy_context_window_tokens);
        let legacy_initialization_digest = context_digest(&initialization)?;
        let part_bytes = HISTORY_PART_BYTES;
        let mut spool = tempfile::NamedTempFile::new()?;
        let mut units = Vec::new();
        append_history_units(&mut spool, &mut units, "incoming-invocation".into(),
            &json!({"action_request":action}), part_bytes)?;
        append_context_source(&mut spool, &mut units, observation, "requesting", part_bytes)?;
        for unit in &mut units {
            let text = read_history_unit(&mut spool, unit)?;
            unit.estimated_tokens = cwr::token_estimation::estimate_tokens(
                &attributed_history_text(unit, &text),
            ).tokens;
        }
        let fixed_input = format!("{}\n{}\n{}", serde_json::to_string(&initialization)?,
            serde_json::to_string(action)?, config.developer_instructions);
        let live_input_tokens = cwr::token_estimation::estimate_tokens(&fixed_input).tokens;
        let mut checkpoint = ContextCheckpoint {
            version: PREPARATION_CHECKPOINT_VERSION,
            provider_store: config.provider_store.map(str::to_owned),
            invocation_ref: action["invocation_ref"].clone(),
            integration_ref: action["integration_ref"].clone(),
            source_digest,
            initialization_digest,
            context_snapshot: crate::adapter::working_context(observation).unwrap_or(observation)["context_snapshot"].clone(),
            thread_id: String::new(),
            confirmed_items: 0,
            pending: false,
            compaction: CompactionState::Idle,
            confirmed_at_last_compaction: 0,
            compactions: 0,
            estimated_input_tokens: live_input_tokens,
            estimator: cwr::token_estimation::ESTIMATOR.into(),
            latest_usage: None,
            model_context_window: None,
            usage_stale: false,
            source_delivery: HistoryDelivery::Injected,
            live_started: false,
        };
        let working = &crate::adapter::working_context(observation).unwrap_or(observation)["working_conversation"];
        let working_ref = working["ref"].as_str().map(str::to_owned);
        let prior_thread = if let Some(state) = working.get("state") {
            let expected_digest = match state["version"].as_u64() {
                Some(1) => &legacy_initialization_digest,
                Some(2 | 3) => &checkpoint.initialization_digest,
                _ => return invalid("Codex private history has an unsupported version"),
            };
            let incompatibility = if state["provider"] != "codex" {
                Some("provider differs")
            } else if !crate::adapter::provider_store_matches(state, config.provider_store) {
                Some("provider store differs")
            } else if state["idle"] != true {
                Some("previous provider conversation is not idle")
            } else if state["working_ref"] != working["ref"] {
                Some("working conversation reference differs")
            } else if state["initialization_digest"].as_str() != Some(expected_digest.as_str()) {
                Some("execution configuration digest differs")
            } else {
                None
            };
            if let Some(reason) = incompatibility {
                return invalid(format!("Codex private history is not a settled compatible conversation: {reason}"));
            }
            if state["version"] == 3
                && state["estimator"].as_str() != Some(checkpoint.estimator.as_str()) {
                return invalid("Codex private history uses a different token estimator");
            }
            let thread_id = state["thread_id"].as_str().filter(|id| !id.trim().is_empty())
                .ok_or("Codex private history has no provider thread")?.to_owned();
            if state["version"] == 3 {
                checkpoint.model_context_window = state["model_context_window"]
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok());
            }
            if state["version"] == 3 && state["usage_stale"] != true {
                if let Some(total_tokens) = state["last_total_tokens"].as_u64()
                    .and_then(|value| usize::try_from(value).ok()) {
                    let turn_id = state["usage_turn_id"].as_str().unwrap_or_default().to_owned();
                    checkpoint.estimated_input_tokens =
                        total_tokens.saturating_add(live_input_tokens);
                    checkpoint.latest_usage = Some(NativeTokenUsage {
                        thread_id: thread_id.clone(), turn_id, total_tokens,
                        model_context_window: checkpoint.model_context_window,
                    });
                }
            }
            Some(thread_id)
        } else { None };
        if let Some(value) = &config.continuation {
            let prior_version = value["version"].as_u64();
            let legacy = prior_version == Some(1);
            let mut value = value.clone();
            if legacy {
                let object = value.as_object_mut().ok_or("Codex continuation must be an object")?;
                object.remove("compacted_items");
                object.remove("working_bytes");
            }
            let prior: ContextCheckpoint = serde_json::from_value(value).map_err(|_| invalid_error(
                "Codex continuation lacks confirmed source coverage; interrupted execution cannot be replayed safely"))?;
            let expected_digest = if legacy { &legacy_initialization_digest } else { &checkpoint.initialization_digest };
            if !matches!(prior.version, 1..=PREPARATION_CHECKPOINT_VERSION)
                || prior.invocation_ref != checkpoint.invocation_ref
                || prior.integration_ref != checkpoint.integration_ref || prior.source_digest != checkpoint.source_digest
                || &prior.initialization_digest != expected_digest
                || prior.context_snapshot != checkpoint.context_snapshot
                || prior.provider_store != checkpoint.provider_store
                || (prior.version >= 3 && prior.estimator != checkpoint.estimator)
                || (prior.version == PREPARATION_CHECKPOINT_VERSION
                    && prior.confirmed_items > units.len())
                || prior.thread_id.trim().is_empty() {
                return invalid("Codex continuation does not match this invocation and its pinned context");
            }
            if prior.live_started {
                return invalid("Codex execution was interrupted after live execution began; automatic replay is unsupported and completed effects are preserved");
            }
            // App-server's read projection cannot prove whether arbitrary raw
            // injected items survived a lost acknowledgment. Reconstruct only the
            // preparation thread from pinned sources. No live effects have run.
            // Earlier versions used different source-boundary projections or
            // fragment encodings. Rebuild their preparation from pinned sources
            // rather than applying an old confirmed-items index to current units.
            if prior.version == PREPARATION_CHECKPOINT_VERSION
                && !prior.pending
                && prior.compaction.is_idle()
            {
                checkpoint = prior;
            }
        }
        let mut plan = Self {
            prior_thread,
            working_ref,
            spool,
            units,
            live_input_tokens,
            checkpoint,
            last_reported_items: None,
            restoration_needs_refresh: false,
            coworker_directory_digest,
            compaction_started_at: None,
            compaction_start_usage: None,
        };
        // A settled thread already contains the previous successful turns.
        // Carry a bounded delta in the next live turn so preparation stays
        // atomic. Oversized deltas retain staged delivery on a private fork,
        // where compaction can run between bounded source units.
        if plan.prior_thread.is_some()
            && plan.checkpoint.source_delivery == HistoryDelivery::Injected
        {
            plan.checkpoint.source_delivery = if plan.live_turn_payload_tokens()
                > plan.compaction_threshold()
            {
                HistoryDelivery::OversizedFork
            } else {
                HistoryDelivery::LiveTurn
            };
        }
        Ok(plan)
    }

    fn resume_thread(&self) -> Option<&str> {
        (!self.checkpoint.thread_id.is_empty()).then_some(self.checkpoint.thread_id.as_str())
    }

    fn provider_resume_thread(&self) -> Option<&str> {
        self.resume_thread().or_else(|| {
            self.delivers_history_with_turn()
                .then_some(self.prior_thread.as_deref())
                .flatten()
        })
    }

    fn provider_fork_thread(&self) -> Option<&str> {
        (self.resume_thread().is_none() && !self.delivers_history_with_turn())
            .then_some(self.prior_thread.as_deref())
            .flatten()
    }

    fn delivers_history_with_turn(&self) -> bool {
        self.checkpoint.source_delivery == HistoryDelivery::LiveTurn
    }

    fn live_turn_payload_tokens(&self) -> usize {
        self.units.iter().fold(self.live_input_tokens, |total, unit| {
            total.saturating_add(unit.estimated_tokens)
        })
    }

    fn report_oversized_fork(&self, handler: &mut impl BoundedActionHandler) {
        if self.checkpoint.source_delivery == HistoryDelivery::OversizedFork {
            handler.on_timing(crate::adapter::AdapterTiming {
                checkpoint: crate::adapter::TimingCheckpoint::OversizedHistoryFork,
                durations_ms: std::collections::BTreeMap::new(),
                context_delivery: None,
                compaction: None,
            });
        }
    }

    fn persist(&self, handler: &mut impl BoundedActionHandler) -> CwrResult<()> {
        handler.on_activity(AdapterActivityEvent::ProviderEvent {
            provider: "codex".into(),
            method: "cwr.adapter_checkpoint".into(),
            data: serde_json::to_value(&self.checkpoint)?,
        })
    }

    fn retain_working_state(&self, handler: &mut impl BoundedActionHandler) -> CwrResult<()> {
        if let Some(working_ref) = &self.working_ref {
            let mut data = json!({"version":3,"provider":"codex","idle":true,
                "working_ref":working_ref,"thread_id":self.checkpoint.thread_id,
                "initialization_digest":self.checkpoint.initialization_digest,
                "invocation_ref":self.checkpoint.invocation_ref,
                "context_snapshot":self.checkpoint.context_snapshot,
                "estimator":self.checkpoint.estimator,
                "last_total_tokens":self.checkpoint.latest_usage.as_ref().map(|usage| usage.total_tokens),
                "model_context_window":self.checkpoint.model_context_window,
                "usage_turn_id":self.checkpoint.latest_usage.as_ref().map(|usage| usage.turn_id.as_str()),
                "usage_stale":self.checkpoint.usage_stale,
                "coworker_directory_digest":self.coworker_directory_digest});
            if let Some(store) = &self.checkpoint.provider_store {
                data["provider_store"] = json!(store);
            }
            handler.on_activity(AdapterActivityEvent::ProviderEvent {
                provider: "codex".into(), method: "cwr.working_state".into(),
                data,
            })?;
        }
        Ok(())
    }

    /// Returns true only once all required history is acknowledged and it is
    /// safe to start the first live turn.
    fn advance(&mut self, writer: &mut impl Write, handler: &mut impl BoundedActionHandler) -> CwrResult<bool> {
        self.report_progress(handler)?;
        if self.delivers_history_with_turn() {
            let newly_confirmed = self.units[self.checkpoint.confirmed_items..]
                .iter()
                .map(|unit| unit.estimated_tokens)
                .sum::<usize>();
            self.checkpoint.confirmed_items = self.units.len();
            self.checkpoint.estimated_input_tokens = self
                .checkpoint
                .estimated_input_tokens
                .saturating_add(newly_confirmed);
            self.persist(handler).map_err(|error| invalid_error(format!(
                "Codex final history checkpoint could not be retained: {error}"
            )))?;
            self.report_progress(handler)?;
            return Ok(true);
        }
        if self.checkpoint.confirmed_items == self.units.len() {
            return Ok(true);
        }
        let unit = &self.units[self.checkpoint.confirmed_items];
        if self.should_compact_before(unit) {
            if self.checkpoint.confirmed_items == self.checkpoint.confirmed_at_last_compaction
                && self.checkpoint.compactions > 0 {
                return invalid("Codex native compaction did not create enough room for the next history item");
            }
            let start_usage = self.checkpoint.latest_usage.clone();
            self.checkpoint.compaction = CompactionState::Requested {
                request_accepted: false,
            };
            self.checkpoint.latest_usage = None;
            self.checkpoint.usage_stale = false;
            self.persist(handler).map_err(|error| invalid_error(format!(
                "Codex checkpoint before context compaction could not be retained: {error}"
            )))?;
            write_json_line(writer, &json!({"id":CONTEXT_COMPACT_ID,
                "method":"thread/compact/start","params":{"threadId":self.checkpoint.thread_id}}))
                .map_err(|error| invalid_error(format!(
                    "Codex context compaction could not be sent to the provider: {error}"
                )))?;
            self.compaction_started_at = Some(Instant::now());
            self.compaction_start_usage = start_usage;
            let sequence = u64::try_from(self.checkpoint.compactions.saturating_add(1))
                .unwrap_or(u64::MAX);
            handler.on_timing(crate::adapter::AdapterTiming {
                checkpoint: crate::adapter::TimingCheckpoint::PreparationCompactionStarted,
                durations_ms: std::collections::BTreeMap::new(),
                context_delivery: None,
                compaction: Some(crate::adapter::AdapterCompactionMetrics {
                    sequence,
                    outcome: crate::adapter::AdapterCompactionOutcome::Running,
                    total_tokens_before: self
                        .compaction_start_usage
                        .as_ref()
                        .and_then(|usage| u64::try_from(usage.total_tokens).ok()),
                    total_tokens_after: None,
                    model_context_window: self
                        .compaction_start_usage
                        .as_ref()
                        .and_then(|usage| usage.model_context_window)
                        .or(self.checkpoint.model_context_window)
                        .and_then(|tokens| u64::try_from(tokens).ok()),
                }),
            });
            return Ok(false);
        }
        let text = read_history_unit(&mut self.spool, unit).map_err(|error| invalid_error(format!(
            "Codex history unit {} could not be read from the preparation spool: {error}",
            self.checkpoint.confirmed_items
        )))?;
        let items = history_items(&self.checkpoint.source_digest, self.checkpoint.confirmed_items, unit, &text);
        self.checkpoint.pending = true;
        self.persist(handler).map_err(|error| invalid_error(format!(
            "Codex history checkpoint before unit {} could not be retained: {error}",
            self.checkpoint.confirmed_items
        )))?;
        write_json_line(writer, &json!({"id":OBSERVATION_INJECT_ID,"method":"thread/inject_items",
            "params":{"threadId":self.checkpoint.thread_id,"items":items}})).map_err(|error|
                invalid_error(format!("Codex history unit {} could not be sent to the provider: {error}",
                    self.checkpoint.confirmed_items)))?;
        Ok(false)
    }

    fn begin_live_turn(&mut self, handler: &mut impl BoundedActionHandler) -> CwrResult<()> {
        self.checkpoint.live_started = true;
        self.persist(handler).map_err(|error| {
            invalid_error(format!(
                "Codex live execution boundary could not be retained: {error}"
            ))
        })
    }

    fn report_progress(&mut self, handler: &mut impl BoundedActionHandler) -> CwrResult<()> {
        let confirmed = self.checkpoint.confirmed_items;
        let total = self.units.len();
        let should_report = self
            .last_reported_items
            .is_none_or(|previous| confirmed == total || confirmed.saturating_sub(previous) >= 10)
            || self.restoration_needs_refresh;
        if !should_report {
            return Ok(());
        }
        use crate::adapter::{ExecutionActivity, ExecutionPhase, ExecutionPhaseState};
        handler.on_activity(AdapterActivityEvent::ExecutionActivity { activity: ExecutionActivity {
            phase: ExecutionPhase::RestoringContext,
            state: if confirmed == total { ExecutionPhaseState::Completed }
                else if self.last_reported_items.is_none() { ExecutionPhaseState::Started }
                else { ExecutionPhaseState::Updated },
            completed_parts: Some(confirmed as u64), total_parts: Some(total as u64),
        }}).map_err(|error| invalid_error(format!(
            "Codex history restoration progress could not be reported: {error}"
        )))?;
        self.last_reported_items = Some(confirmed);
        self.restoration_needs_refresh = false;
        Ok(())
    }

    fn injected(&mut self, handler: &mut impl BoundedActionHandler) -> CwrResult<()> {
        if !self.checkpoint.pending {
            return invalid("Codex acknowledged unexpected context injection");
        }
        let tokens = self.units[self.checkpoint.confirmed_items].estimated_tokens;
        self.checkpoint.confirmed_items += 1;
        self.checkpoint.estimated_input_tokens = self.checkpoint.estimated_input_tokens
            .saturating_add(tokens);
        self.checkpoint.pending = false;
        // The provider's last token report predates this newly accepted history
        // unit, so it is not a current pre-compaction measurement.
        self.checkpoint.latest_usage = None;
        self.persist(handler).map_err(|error| invalid_error(format!(
            "Codex history acknowledgement for unit {} could not be retained: {error}",
            self.checkpoint.confirmed_items - 1
        )))
    }

    fn observe_token_usage(&mut self, message: &Value) {
        let Some(thread_id) = message.pointer("/params/threadId").and_then(Value::as_str)
            .filter(|thread| *thread == self.checkpoint.thread_id) else { return; };
        let Some(turn_id) = message.pointer("/params/turnId").and_then(Value::as_str) else { return; };
        let Some(total_tokens) = message.pointer("/params/tokenUsage/last/totalTokens")
            .and_then(Value::as_u64).and_then(|value| usize::try_from(value).ok()) else { return; };
        let model_context_window = message.pointer("/params/tokenUsage/modelContextWindow")
            .and_then(Value::as_u64).and_then(|value| usize::try_from(value).ok());
        self.checkpoint.latest_usage = Some(NativeTokenUsage {
            thread_id: thread_id.to_owned(), turn_id: turn_id.to_owned(),
            total_tokens, model_context_window,
        });
        if model_context_window.is_some() {
            self.checkpoint.model_context_window = model_context_window;
        }
        self.checkpoint.estimated_input_tokens = total_tokens;
        self.checkpoint.usage_stale = false;
    }

    fn observe_live_message(&mut self, message: &Value, active_turn_id: Option<&str>) {
        if message.get("method").and_then(Value::as_str) == Some("thread/tokenUsage/updated") {
            let event_turn_id = message.pointer("/params/turnId").and_then(Value::as_str);
            if event_turn_id.is_some() && event_turn_id == active_turn_id {
                self.observe_token_usage(message);
            }
        } else if self.checkpoint.latest_usage.is_some()
            && message.get("method").and_then(Value::as_str) == Some("item/completed")
            && matches!(message.pointer("/params/item/type").and_then(Value::as_str),
                Some("agentMessage" | "reasoning")) {
            self.checkpoint.usage_stale = true;
        }
    }

    fn mark_usage_stale(&mut self) {
        if self.checkpoint.latest_usage.is_some() {
            self.checkpoint.usage_stale = true;
        }
    }
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod history_tests;
