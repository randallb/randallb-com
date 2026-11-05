pub const PROCESS_ADAPTER_PROTOCOL_REF: &str =
    "cwp://org.coworkerprotocol/2026-09/protocols/process_adapter";
pub const DECISIONS_ADAPTER_REF: &str = "cwp://org.coworkerprotocol/2026-09/adapters/decisions";
pub const CODEX_ADAPTER_REF: &str = "cwp://org.coworkerprotocol/2026-09/adapters/codex";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterReadinessStatus {
    Ready,
    SetupRequired,
    TemporarilyUnavailable,
}

/// A symbolic setup operation offered by the trusted adapter implementation.
/// The host resolves the operation; portable resources never supply commands or URLs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterRepairOffer {
    pub kind: AdapterRepairKind,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterRepairKind {
    ProviderLogin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterReadiness {
    pub status: AdapterReadinessStatus,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<AdapterRepairOffer>,
}

impl AdapterReadiness {
    pub fn is_ready(&self) -> bool {
        self.status == AdapterReadinessStatus::Ready
    }
}
pub const CLAUDE_ADAPTER_REF: &str = "cwp://org.coworkerprotocol/2026-09/adapters/claude";
pub const OPEN_RESPONSES_ADAPTER_REF: &str =
    "cwp://org.coworkerprotocol/2026-09/adapters/open-responses";
pub const LOOPBACK_ADAPTER_REF: &str = "cwp://org.coworkerprotocol/2026-09/adapters/loopback";
pub const RNG_ADAPTER_REF: &str = "cwp://org.coworkerprotocol/2026-09/adapters/rng";
pub const SH_ADAPTER_REF: &str = "cwp://org.coworkerprotocol/2026-09/adapters/sh";
pub const SLACK_ADAPTER_REF: &str = "cwp://org.coworkerprotocol/2026-09/adapters/slack";

const ADAPTER_EXIT_TIMEOUT: Duration = Duration::from_secs(2);
const ADAPTER_POLL_INTERVAL: Duration = Duration::from_millis(20);
pub const MAX_ACTION_VALUE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PROTOCOL_LINE_BYTES: usize = 32 * 1024 * 1024;
// Ordinary delivery is bounded independently by frame count and encoded bytes.
// The reader may retain one additional terminal frame outside that queue so a
// progress backlog cannot hide completion.
const PROTOCOL_QUEUE_BYTES: usize = 32 * 1024 * 1024;
const PROTOCOL_QUEUE_FRAMES: usize = 4;
pub const MODEL_OUTPUT_MAILBOX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ResolvedProcessAdapter {
    executable: PathBuf,
    arguments: Vec<OsString>,
    current_directory: Option<PathBuf>,
    environment: BTreeMap<OsString, OsString>,
    shutdown_grace_period: Duration,
}

impl ResolvedProcessAdapter {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            arguments: Vec::new(),
            current_directory: None,
            environment: BTreeMap::new(),
            shutdown_grace_period: Duration::ZERO,
        }
    }

    pub fn with_argument(mut self, argument: impl Into<OsString>) -> Self {
        self.arguments.push(argument.into());
        self
    }

    /// Allow a trusted adapter to clean up provider children on SIGTERM before
    /// the host kills its process group. Existing adapters retain immediate kill.
    pub fn with_shutdown_grace_period(mut self, duration: Duration) -> Self {
        self.shutdown_grace_period = duration;
        self
    }

    #[cfg(test)]
    pub(crate) fn shutdown_grace_period(&self) -> Duration {
        self.shutdown_grace_period
    }

    pub fn with_current_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.current_directory = Some(directory.into());
        self
    }

    pub fn with_environment(
        mut self,
        name: impl Into<OsString>,
        value: impl Into<OsString>,
    ) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AdapterInvocation {
    /// Conversational model requests may continue their actor's private history.
    #[serde(default)]
    pub conversation_continuity: bool,
    #[serde(default)]
    pub execution_scope: crate::action_contract::ExecutionScope,
    #[serde(default)]
    pub response_mode: crate::action_contract::ResponseMode,
    pub invocation_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_actor_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_run_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration_ref: Option<String>,
    /// Host-retained execution binding. Removed before process transport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_fingerprint: Option<String>,
    /// Provider state excludes live CWR authorization; retries still use the full route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_fingerprint: Option<String>,
    /// The activated executable route selected when this invocation was admitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_route: Option<ResolvedProcessAdapterRoute>,
    /// Required for newly admitted local work; missing historical records never imply approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_inputs: Option<crate::input_delivery::InvocationInputs>,
    pub action_name: String,
    pub input: Value,
    pub output_schema: Value,
    pub thread: Value,
    pub adapter_params: Value,
    pub integration_params: Value,
    pub actor_integration_params: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authority: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_operations: Vec<AdapterHostOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<Value>,
}

impl AdapterInvocation {
    pub fn new(
        invocation_ref: impl Into<String>,
        action_name: impl Into<String>,
        input: Value,
        output_schema: Value,
    ) -> Self {
        Self {
            conversation_continuity: false,
            execution_scope: Default::default(),
            response_mode: Default::default(),
            invocation_ref: invocation_ref.into(),
            source_actor_ref: None,
            model_run_ref: None,
            integration_ref: None,
            route_fingerprint: None,
            provider_fingerprint: None,
            retained_route: None,
            local_inputs: Some(Default::default()),
            action_name: action_name.into(),
            input,
            output_schema,
            thread: json!({}),
            adapter_params: json!({}),
            integration_params: json!({}),
            actor_integration_params: json!({}),
            authority: Vec::new(),
            host_operations: Vec::new(),
            continuation: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AdapterHostOperation {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AdapterCall {
    pub invocation_ref: String,
    pub call_id: String,
    pub operation: String,
    pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdapterHostMessage {
    Invoke {
        protocol: String,
        invocation: Box<AdapterInvocation>,
    },
    CallResult {
        invocation_ref: String,
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<AdapterError>,
    },
    ActivityResult {
        invocation_ref: String,
        sequence: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<AdapterError>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AdapterError {
    pub code: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_string"
    )]
    pub message: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterProgress {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::v2026_09::invocation::progress::present")]
    pub progress: Option<crate::v2026_09::invocation::ActionProgress>,
    #[serde(default)]
    pub published: bool,
    pub invocation_ref: String,
    pub sequence: u64,
    pub status: u16,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterActivity {
    pub invocation_ref: String,
    pub sequence: u64,
    pub event: AdapterActivityEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterCompletedCommentary {
    pub model_run_ref: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub response_or_turn_id: String,
    pub item_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AdapterModelOutput {
    pub model_run_ref: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub response_or_turn_id: String,
    pub phase: crate::open_responses::v2026_04_24::AssistantPhase,
    pub event: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AdapterLiveOutput {
    pub invocation_ref: String,
    pub model_run_ref: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub response_or_turn_id: String,
    pub item_id: String,
    pub phase: crate::open_responses::v2026_04_24::AssistantPhase,
    pub sequence_number: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct AdapterClosedModelRun {
    pub invocation_ref: String,
    pub model_run_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterLiveOutputSnapshot {
    pub revision: u64,
    pub pending_bytes: usize,
    pub output: Option<AdapterLiveOutput>,
    pub closed_model_runs: Vec<AdapterClosedModelRun>,
}

include!("model/model_output.rs");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdapterActivityEvent {
    ExecutionActivity {
        activity: ExecutionActivity,
    },
    ProviderThread {
        provider: String,
        thread_id: String,
    },
    ProviderEvent {
        provider: String,
        method: String,
        data: Value,
    },
    CompletedCommentary {
        commentary: AdapterCompletedCommentary,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AdapterTerminal {
    pub invocation_ref: String,
    pub sequence: u64,
    pub status: u16,
    pub summary: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_value"
    )]
    pub result: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_adapter_error"
    )]
    pub error: Option<AdapterError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdapterEvent {
    Timing {
        invocation_ref: String,
        model_run_ref: Option<String>,
        timing: AdapterTiming,
    },
    Activity {
        invocation_ref: String,
        sequence: u64,
        event: AdapterActivityEvent,
    },
    ModelOutput {
        invocation_ref: String,
        output: AdapterModelOutput,
    },
    Progress {
        #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::v2026_09::invocation::progress::present")]
        progress: Option<crate::v2026_09::invocation::ActionProgress>,
        #[serde(default)]
        published: bool,
        invocation_ref: String,
        sequence: u64,
        status: u16,
        summary: String,
    },
    Result {
        invocation_ref: String,
        sequence: u64,
        status: u16,
        summary: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_present_value"
        )]
        result: Option<Value>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_present_adapter_error"
        )]
        error: Option<AdapterError>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        continuation: Option<Value>,
    },
    Call {
        invocation_ref: String,
        call_id: String,
        operation: String,
        input: Value,
    },
}

fn deserialize_present_value<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

fn deserialize_present_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

fn deserialize_present_adapter_error<'de, D>(
    deserializer: D,
) -> Result<Option<AdapterError>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    AdapterError::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProcessAdapterOutcome {
    Completed(Box<AdapterTerminal>),
    Stopped,
}

/// Process diagnostics retained when an adapter ends outside the protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessAdapterFailure {
    pub message: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout_state: String,
    pub stderr_tail: String,
    pub stderr_total_bytes: u64,
    pub stderr_truncated: bool,
    pub stderr_complete: bool,
    pub stderr_capture_error: Option<String>,
}

impl fmt::Display for ProcessAdapterFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProcessAdapterFailure {}

#[derive(Debug, Clone, PartialEq)]
pub enum AdapterHostCallOutcome {
    Returned(Value),
}

#[derive(Debug, Default)]
struct ProcessAdapterStopState {
    requested: AtomicBool,
    process_groups: Mutex<BTreeMap<i32, ProcessGroupShutdown>>,
    model_output_run: Mutex<Option<ModelOutputRunRegistration>>,
}

#[derive(Debug, Clone)]
struct ModelOutputRunRegistration {
    mailbox: AdapterModelOutputMailbox,
    invocation_ref: String,
    model_run_ref: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessAdapterStopControl {
    state: Arc<ProcessAdapterStopState>,
}

impl ProcessAdapterStopControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stop(&self) {
        self.state.requested.store(true, Ordering::Release);
        let model_output_run = { self.lock_model_output_run().clone() };
        if let Some(run) = model_output_run {
            run.mailbox
                .close_run(&run.invocation_ref, run.model_run_ref.as_deref());
        }
        let groups: Vec<_> = self.lock_process_groups().keys().copied().collect();
        for group in groups {
            let _ = self.request_process_group_stop(group);
        }
    }

    pub(crate) fn is_requested(&self) -> bool {
        self.state.requested.load(Ordering::Acquire)
    }

    fn register_model_output_run(
        &self,
        mailbox: AdapterModelOutputMailbox,
        invocation_ref: String,
        model_run_ref: Option<String>,
    ) {
        let registration = ModelOutputRunRegistration {
            mailbox,
            invocation_ref,
            model_run_ref,
        };
        *self.lock_model_output_run() = Some(registration.clone());
        if self.is_requested() {
            registration.mailbox.close_run(
                &registration.invocation_ref,
                registration.model_run_ref.as_deref(),
            );
        }
    }

    fn clear_model_output_run(&self) {
        self.lock_model_output_run().take();
    }

    fn register_process_group(&self, process_group: i32, grace: Duration) -> CwrResult<()> {
        let mut process_groups = self.lock_process_groups();
        if process_groups.contains_key(&process_group) {
            return protocol_failure(format!(
                "adapter process group {process_group} is already registered"
            ));
        }
        process_groups.insert(process_group, ProcessGroupShutdown::new(grace));
        drop(process_groups);
        if self.state.requested.load(Ordering::Acquire) {
            self.request_process_group_stop(process_group)?;
        }
        Ok(())
    }

    fn lock_process_groups(&self) -> MutexGuard<'_, BTreeMap<i32, ProcessGroupShutdown>> {
        self.state
            .process_groups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_model_output_run(&self) -> MutexGuard<'_, Option<ModelOutputRunRegistration>> {
        self.state
            .model_output_run
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

struct ModelOutputRunGuard {
    mailbox: AdapterModelOutputMailbox,
    invocation_ref: String,
    model_run_ref: Option<String>,
    stop: ProcessAdapterStopControl,
}

impl Drop for ModelOutputRunGuard {
    fn drop(&mut self) {
        self.mailbox
            .close_run(&self.invocation_ref, self.model_run_ref.as_deref());
        self.stop.clear_model_output_run();
    }
}

include!("model/tests.rs");
/// The shared source whose observation is covered by continuing private work.
/// An own-conversation invocation may wrap it in a separate collaboration context.
pub fn working_context(thread: &Value) -> Option<&Value> {
    if thread.get("working_conversation").is_some() {
        Some(thread)
    } else {
        thread.get("requesting_context").and_then(working_context)
    }
}
