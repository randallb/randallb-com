/// Small, content-free timing checkpoints. Durations use the producer's monotonic clock.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterTiming {
    pub checkpoint: TimingCheckpoint,
    pub durations_ms: BTreeMap<TimingDuration, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_delivery: Option<AdapterContextDeliveryMetrics>,
    /// Details for one provider context compaction observation. All token counts
    /// are optional because providers do not report them consistently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<AdapterCompactionMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterContextDeliveryMetrics {
    pub source_bytes: u64,
    pub chunks: u64,
    pub max_chunk_frame_bytes: usize,
    pub invoke_frame_bytes: usize,
    pub protocol_queue_limit_bytes: usize,
}

/// Content-free dimensions for one context compaction observation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterCompactionMetrics {
    /// One-based sequence within preparation or the active turn.
    pub sequence: u64,
    /// Whether this starts, completes, or fails a compaction. A `running`
    /// observation without a later terminal observation is incomplete.
    pub outcome: AdapterCompactionOutcome,
    /// Provider-reported total context tokens before compaction, when known.
    pub total_tokens_before: Option<u64>,
    /// Provider-reported total context tokens after compaction, when known.
    pub total_tokens_after: Option<u64>,
    /// Provider-reported model context window, when known.
    pub model_context_window: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterCompactionOutcome {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimingCheckpoint {
    AdapterSpawned,
    ContextDelivered,
    AdapterPrepared,
    ProviderSpawned,
    OversizedHistoryFork,
    PreparationCompactionStarted,
    PreparationCompactionCompleted,
    PreparationCompactionFailed,
    StartupComplete,
    TurnAccepted,
    FirstActivity,
    FirstText,
    LiveCompactionStarted,
    LiveCompactionCompleted,
    LiveCompactionFailed,
    ProviderEnded,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TimingDuration {
    AdapterSpawn,
    ContextDelivery,
    AdapterPreparation,
    ProviderSpawn,
    ContextPreparation,
    ProviderInitialize,
    ThreadSetup,
    ContextInjection,
    ContextCompaction,
    HostAcknowledgement,
    ProviderStartup,
    TurnAcceptance,
    FirstActivity,
    FirstText,
    ProviderElapsed,
}

impl AdapterTiming {
    pub fn single(checkpoint: TimingCheckpoint, duration: TimingDuration, elapsed_ms: u64) -> Self {
        Self {
            checkpoint,
            durations_ms: BTreeMap::from([(duration, elapsed_ms)]),
            context_delivery: None,
            compaction: None,
        }
    }
}

/// Receives at most a few checkpoints per attempt on the protocol reader thread.
/// Callbacks must not block; the runtime callback only tries a bounded queue.
#[derive(Clone, Default)]
pub struct AdapterTimingSink(Option<Arc<dyn Fn(AdapterTiming) + Send + Sync>>);

pub struct AdapterOutputSinks<'a> {
    pub model_output: &'a AdapterModelOutputMailbox,
    pub timing: &'a AdapterTimingSink,
}

impl AdapterTimingSink {
    pub fn new(callback: impl Fn(AdapterTiming) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(callback)))
    }

    pub fn record(&self, timing: AdapterTiming) {
        if let Some(callback) = &self.0 {
            callback(timing);
        }
    }
}
