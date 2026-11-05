#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationIngress {
    /// Opaque host product ownership, independent of participants and parentage.
    /// This annotation does not grant access or change routing authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_context_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_refs: Vec<String>,
    pub integration_ref: String,
    pub provider_event_ref: String,
    pub author_actor_ref: String,
    pub conversation: ConversationIngressConversation,
    pub addressing: ConversationIngressAddressing,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationIngressConversation {
    /// An explicit host conversation create, regardless of its first message's addressing.
    Create,
    /// A standalone action request with no existing requesting conversation.
    CreateForAction,
    Continue {
        conversation_ref: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationIngressAddressing {
    Addressed {
        actor_refs: Vec<String>,
    },
    Action {
        actor_ref: String,
        activation: String,
        action_name: String,
        input: Value,
    },
    UnresolvedLeadingHandle {
        handle: String,
    },
    Unaddressed,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationMessage {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Box<ConversationProgress>")]
    pub progress: Option<Box<ConversationProgress>>,
    pub message_ref: String,
    pub author_actor_ref: String,
    pub addressed_actor_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ConversationAddressingMode")]
    pub addressing_mode: Option<ConversationAddressingMode>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visualizations: Vec<ConversationVisualization>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub reply_to_message_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ConversationActionEvent")]
    pub action: Option<ConversationActionEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationVisualization {
    pub marker_index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub artifact_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationVisualizationArtifact {
    pub artifact_ref: String,
    pub html: String,
}

/// A published update retains its source without becoming an action result.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationProgress {
    pub invocation_ref: String,
    pub source_activity_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ActionResponseStatus")]
    pub status: Option<ActionResponseStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostConversationExchange {
    pub conversation: RetainedConversation,
    pub request_message_ref: String,
    pub response_message_ref: String,
}

#[derive(
    Debug, Clone, Copy, Default, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq,
)]
#[serde(rename_all = "snake_case")]
pub enum ConversationAddressingMode {
    #[default]
    Implicit,
    Explicit,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationActionEvent {
    Call {
        activation: String,
        input: Value,
    },
    Result {
        activation: String,
        result: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "ConversationActionOutcome")]
        outcome: Option<ConversationActionOutcome>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationActionOutcome {
    Succeeded,
    Failed,
}

pub struct ConversationHostActionCall<'a> {
    pub conversation_ref: &'a str,
    pub caller_invocation_ref: &'a str,
    pub host_call_id: &'a str,
    pub nested_invocation_ref: &'a str,
    pub caller_actor_ref: &'a str,
    pub target_actor_ref: &'a str,
    pub activation: &'a str,
    pub input: Value,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetainedConversation {
    /// Opaque host product ownership, independent of participants and parentage.
    /// This annotation does not grant access or change routing authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_context_ref: Option<String>,
    pub conversation_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_conversation_ref: Option<String>,
    /// Creation-time sidebar eligibility; absent on legacy retained records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_eligible: Option<bool>,
    pub participant_actor_refs: Vec<String>,
    pub messages: Vec<ConversationMessage>,
    /// The root of the conversation's canonical publication timeline. Absence
    /// identifies a legacy conversation; a present empty connection is
    /// authoritative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_connection_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activity: Vec<ConversationActivity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationActivity {
    pub activity_ref: String,
    pub sequence: u64,
    #[serde(default)]
    pub occurred_at_unix_ms: u64,
    pub event: ConversationActivityEvent,
}

/// Display fields for one tool operation, without provider routing or arguments.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkActivity {
    pub activity_ref: String,
    pub kind: String,
    pub label: String,
    pub state: String,
    pub started_at_unix_ms: u64,
    /// The nested invocation represented by this activity, when the work is a
    /// coworker-to-coworker action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub child_invocation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "i64")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<WorkFileChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkFileChange {
    pub path: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub diff: Option<String>,
}

/// The authoritative relationship between a child invocation and a parent
/// that is waiting for it, without exposing the child's response payload.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkInvocationWait {
    pub waiter_invocation_ref: String,
    pub awaited_invocation_ref: String,
    pub call_id: String,
    pub state: InvocationWaitState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub resume_after_activity_sequence: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkUpdateTextKind {
    Commentary,
    Progress,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationActivityEvent {
    /// Timestamp and identity for interleaving canonical messages with work.
    MessageRecorded {
        message_ref: String,
    },
    /// Associates a copied public response with the work event that produced it.
    MessageWorkAssociation {
        message_ref: String,
        activity_ref: String,
    },
    /// Public lineage for a conversation copied through one selected message.
    ConversationForkedFrom {
        /// Destination history refs mapped to their immediate source records.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        inherited_refs: BTreeMap<String, String>,
        source_conversation_ref: String,
        source_message_ref: String,
        forked_by_actor_ref: String,
        operation_id: String,
        forked_at_unix_ms: u64,
        #[serde(default)]
        lineage_depth: u64,
    },
    ExecutionActivity {
        activity: crate::adapter::ExecutionActivity,
    },
    WorkPhase {
        invocation_ref: String,
        actor_ref: String,
        activity: crate::adapter::ExecutionActivity,
    },
    /// Public lifecycle facts from the invocation ledger. This is a snapshot;
    /// consumers derive stable start/finish events from its timestamps.
    WorkLifecycle {
        invocation_ref: String,
        actor_ref: String,
        source_message_ref: String,
        parent_invocation_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        awaited_by_invocation_ref: Option<String>,
        state: crate::v2026_09::invocation::ActionInvocationState,
        admitted_at_unix_ms: u64,
        started_at_unix_ms: Option<u64>,
        stop_requested_at_unix_ms: Option<u64>,
        finished_at_unix_ms: Option<u64>,
    },
    /// Provider-neutral presentation of a coworker's retained work on a message.
    WorkUpdate {
        invocation_ref: String,
        source_message_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        parent_invocation_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Box<WorkInvocationWait>")]
        wait: Option<Box<WorkInvocationWait>>,
        actor_ref: String,
        status: ActionResponseStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "WorkUpdateTextKind")]
        text_kind: Option<WorkUpdateTextKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Box<WorkActivity>")]
        activity: Option<Box<WorkActivity>>,
    },
    TurnStarted {
        invocation_ref: String,
        source_message_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Value")]
        invocation: Option<Value>,
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
    Progress {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::v2026_09::invocation::progress::present"
        )]
        #[schemars(with = "crate::v2026_09::invocation::ActionProgress")]
        progress: Option<crate::v2026_09::invocation::ActionProgress>,
        #[serde(deserialize_with = "deserialize_progress_status")]
        #[schemars(schema_with = "crate::v2026_09::invocation::progress_status_schema")]
        status: ActionResponseStatus,
        summary: String,
    },
    CommentaryProgress {
        model_run_ref: String,
        provider_integration_ref: String,
        provider: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        thread_id: Option<String>,
        response_or_turn_id: String,
        item_id: String,
        text: String,
    },
    HostCall {
        call_id: String,
        operation: String,
        input: Value,
    },
    HostActionStarted {
        call_id: String,
        nested_invocation_ref: String,
        call_message_ref: String,
    },
    HostResult {
        call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Value")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        error: Option<String>,
    },
    TurnCompleted {
        #[serde(deserialize_with = "deserialize_completed_status")]
        #[schemars(schema_with = "crate::v2026_09::invocation::completed_status_schema")]
        status: ActionResponseStatus,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Value")]
        result: Option<Value>,
    },
    TurnAttemptFailed {
        error: String,
    },
    TurnFailed {
        #[serde(deserialize_with = "deserialize_failure_status")]
        #[schemars(schema_with = "crate::v2026_09::invocation::failure_status_schema")]
        status: ActionResponseStatus,
        error: String,
    },
    TurnStopped {
        #[serde(deserialize_with = "deserialize_stopped_status")]
        #[schemars(schema_with = "crate::v2026_09::invocation::stopped_status_schema")]
        status: ActionResponseStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        reason: Option<String>,
    },
}

fn deserialize_progress_status<'de, D>(deserializer: D) -> Result<ActionResponseStatus, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_activity_status(deserializer, ActionResponseStatus::is_progress, "progress")
}

fn deserialize_completed_status<'de, D>(deserializer: D) -> Result<ActionResponseStatus, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_activity_status(
        deserializer,
        ActionResponseStatus::is_completed,
        "completed",
    )
}

fn deserialize_failure_status<'de, D>(deserializer: D) -> Result<ActionResponseStatus, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_activity_status(
        deserializer,
        |status| status.is_blocked() || status.is_failed(),
        "blocked or failed",
    )
}

fn deserialize_stopped_status<'de, D>(deserializer: D) -> Result<ActionResponseStatus, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_activity_status(deserializer, ActionResponseStatus::is_stopped, "stopped")
}

fn deserialize_activity_status<'de, D>(
    deserializer: D,
    allowed: impl FnOnce(ActionResponseStatus) -> bool,
    category: &str,
) -> Result<ActionResponseStatus, D::Error>
where
    D: Deserializer<'de>,
{
    let status = ActionResponseStatus::deserialize(deserializer)?;
    if !allowed(status) {
        return Err(D::Error::custom(format!(
            "activity status {} is not {category}",
            status.as_u16()
        )));
    }
    Ok(status)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordCommentaryProgressOutcome {
    Recorded(ConversationActivity),
    Existing(ConversationActivity),
    IgnoredStaleModelRun,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationRuntimeState {
    version: String,
    runtime_root_ref: String,
    conversations: crate::journal::JournalVec<RetainedConversation>,
    // Present only in a managed resident that deferred closed history. These
    // entries have complete metadata but their message/activity vectors must
    // be hydrated before they are exposed or mutated.
    #[serde(skip)]
    cold_conversation_refs: std::collections::BTreeSet<String>,
    #[serde(default)]
    files: crate::journal::JournalMap<String, RetainedFileContent>,
    #[serde(default)]
    visualization_artifacts: crate::journal::JournalMap<String, ConversationVisualizationArtifact>,
    admissions: crate::journal::JournalVec<ConversationAdmissionRecord>,
    #[serde(default, skip_serializing_if = "ActionInvocationLedger::is_empty")]
    invocation_ledger: ActionInvocationLedger,
    #[serde(default, skip_serializing_if = "ConversationTimelineStore::is_empty")]
    timeline_store: ConversationTimelineStore,
    #[serde(default)]
    streaming_messages: crate::journal::JournalMap<String, ConversationStreamingMessage>,
    #[serde(default)]
    streaming_runs: crate::journal::JournalMap<String, streaming::StreamingRun>,
    #[serde(default)]
    tool_attempts: crate::journal::JournalMap<String, tools::ToolAttempt>,
    #[serde(default)]
    tool_operations: crate::journal::JournalMap<String, tools::ToolOperation>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    invocation_execution_conversation_refs: crate::journal::JournalMap<String, String>,
    #[serde(default)]
    upstream_participant_versions: crate::journal::JournalMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    protected_result_children: crate::journal::JournalMap<String, std::collections::BTreeSet<String>>,
}

pub struct ConversationAdmissionStore {
    pub(crate) metrics: Option<crate::metrics::MetricsRecorder>,
    pub(crate) context_policy_configured: bool,
    pub(crate) context_policy: Option<crate::v2026_09::upstream::ConversationContextPolicy>,
    pub(crate) response_graph: Option<ResolvedCwrGraph>,
    state_path: PathBuf,
    commit_revision: i64,
    pending_authority_guards: Vec<cwpdb_store::conversation::AuthorityGuard>,
    backend: std::sync::Arc<std::sync::Mutex<backend::ConversationBackend>>,
    admission_indices_by_conversation: BTreeMap<String, Vec<usize>>,
    timeline_rollout: Option<ConversationTimelineRollout>,
    state: ConversationRuntimeState,
}
