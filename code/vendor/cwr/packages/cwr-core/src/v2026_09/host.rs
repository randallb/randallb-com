//! Provider-neutral process boundary for conversation-owning CWR hosts.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    intrinsic_cwr::IntrinsicCwrResponse,
    open_responses::v2026_04_24::AssistantPhase,
    v2026_09::{
        conversation::{
            normalize_conversation_title, ConversationActivityEvent, ConversationAdmissionStore,
            ConversationRecordPage, ConversationRecordReadRequest, ConversationSummary,
            ConversationSummaryPage, ConversationSummaryPageRequest, ConversationTimelineAnchor,
            ConversationVisualizationArtifact, RetainedConversation,
            MAX_CONVERSATION_RECORD_PAGE_SIZE, MAX_CONVERSATION_SUMMARY_PAGE_SIZE,
        },
        graph::ResolvedCwrGraph,
        invocation::{
            ActionInvocationState, ActionResponse, InvocationOrigin, InvocationWaitState,
        },
        local::{LocalActor, LocalActorIdentity, LocalCatalog},
    },
    CwrRef, CwrResult,
};

include!("host/capabilities.rs");
include!("host/work.rs");
include!("host/synchronous.rs");

pub const HOST_COMMAND_VERSION: u32 = 1;

fn is_false(value: &bool) -> bool {
    !*value
}

/// A conversation-state operation that must not add activity to the conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationControl {
    Rename {
        title: String,
    },
    Archive,
    Restore,
    Export,
    Import {
        path: String,
        import_id: String,
    },
    Fork {
        message_ref: String,
        fork_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostCommand {
    Upstream {
        version: u32,
        request_id: String,
        command: super::upstream::UpstreamCommand,
    },
    InspectWorkspace {
        version: u32,
        request_id: String,
    },
    SelectActor {
        version: u32,
        request_id: String,
        actor_ref: String,
    },
    ReadRootConversations {
        version: u32,
        request_id: String,
        source_ref: String,
        page: ConversationSummaryPageRequest,
        timeout_ms: u64,
    },
    ReadConversationRecords {
        version: u32,
        request_id: String,
        source_ref: String,
        read: ConversationRecordReadRequest,
        timeout_ms: u64,
    },
    ReadVisualization {
        version: u32,
        request_id: String,
        source_ref: String,
        artifact_ref: String,
    },
    CreateConversation {
        version: u32,
        request_id: String,
        source_ref: String,
        author_actor_ref: String,
        text: String,
    },
    /// Reload this host session's bound runtime for its selected actor.
    ReloadGraph {
        version: u32,
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_ref: Option<String>,
    },
    ReplyPersonRequest {
        version: u32,
        request_id: String,
        source_ref: String,
        conversation_ref: String,
        invocation_ref: String,
        text: String,
    },
    ContinueConversation {
        version: u32,
        request_id: String,
        source_ref: String,
        conversation_ref: String,
        author_actor_ref: String,
        text: String,
        #[serde(default, skip_serializing_if = "is_false")]
        recovery_only: bool,
    },
    ExecuteConversationControl {
        version: u32,
        request_id: String,
        source_ref: String,
        conversation_ref: Option<String>,
        requesting_actor_ref: String,
        control: ConversationControl,
    },
    RecordHostExchange {
        version: u32,
        request_id: String,
        source_ref: String,
        requesting_actor_ref: String,
        host_actor_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_ref: Option<String>,
        request_text: String,
        response_text: String,
    },
    AttachmentCreated {
        version: u32,
        request_id: String,
        attachment_ref: String,
    },
    AdmitIngress {
        version: u32,
        request_id: String,
        ingress: Box<super::conversation::ConversationIngress>,
        actor_integration_ref: Option<String>,
        attachment: Option<super::service::RuntimeIngressAttachment>,
    },
    InvokeAction {
        version: u32,
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_ref: Option<String>,
        author_actor_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_ref: Option<String>,
        activation: String,
        input: Value,
    },
    StopConversation {
        version: u32,
        request_id: String,
        source_ref: String,
        conversation_ref: String,
        requesting_actor_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation_ref: Option<String>,
    },
    SetConversationLiveStatus {
        version: u32,
        request_id: String,
        source_ref: String,
        conversation_ref: String,
        subject_actor_ref: String,
        publisher_session_id: String,
        sequence: u64,
        target_actor_refs: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<ConversationLiveStatusName>,
        lease_ms: u64,
    },
}

include!("host/command.rs");

include!("host/receipt.rs");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostWorkspace {
    pub workspace_ref: String,
    #[serde(alias = "configured_loop_ref")]
    pub runtime_ref: String,
    pub actors: Vec<HostActor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<crate::v2026_09::graph::GraphLoadWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostActor {
    pub actor_ref: String,
    pub coworker_ref: String,
    pub handle: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<crate::v2026_09::graph::CoworkerCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HostActionCatalog {
    #[serde(default)]
    pub known_actors: Vec<HostActor>,
    #[serde(default)]
    pub retained_actors: Vec<HostActor>,
    #[serde(default)]
    pub default_conversation_target_ref: Option<String>,
    pub actors: Vec<HostActor>,
    pub actions: Vec<HostAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HostAction {
    pub activation: String,
    pub actor_ref: String,
    pub action_name: String,
    pub title: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
}

include!("host/workspace.rs");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationSource {
    pub source_ref: String,
    pub runtime_ref: String,
    pub display_name: String,
    pub capabilities: ConversationSourceCapabilities,
    /// Local, host-owned path to Workloop's content-free performance database.
    /// It is absent for integrations that do not expose local performance data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub performance_database_path: Option<String>,
    /// Identity reported by the connected resident, never by its bridge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_identity: Option<BuildIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationLiveOutput {
    pub invocation_ref: String,
    /// The durable claim generation for this invocation. Consumers must only
    /// attach output to a matching active execution.
    pub generation: u64,
    pub target_actor_ref: String,
    pub phase: AssistantPhase,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLiveStatusName {
    Composing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationLiveStatus {
    pub subject_actor_ref: String,
    pub publisher_session_id: String,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_actor_refs: Vec<String>,
    pub status: ConversationLiveStatusName,
    pub expires_at_unix_ms: u64,
}

include!("host/turn_status.rs");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostEvent {
    /// Planned development replacement, emitted after active work finishes.
    RuntimeRestarting {
        generation: String,
    },
    UpstreamRegistered {
        request_id: String,
        provider_session_ref: String,
    },
    Upstream {
        event: super::upstream::UpstreamEvent,
    },
    /// Saved locally for delayed delivery; the executing host has not accepted it.
    CommandQueued {
        request_id: String,
    },
    WorkspaceInspected {
        request_id: String,
        workspace: HostWorkspace,
        default_actor_ref: Option<String>,
    },
    ActorSelected {
        request_id: String,
        actor: HostActor,
        catalog: HostActionCatalog,
    },
    CatalogUpdated {
        actor: HostActor,
        catalog: HostActionCatalog,
    },
    Ready {
        sources: Vec<ConversationSource>,
    },
    ConversationSnapshot {
        source_ref: String,
        conversations: Vec<RetainedConversation>,
    },
    RootConversationsPage {
        request_id: String,
        source_ref: String,
        page: ConversationSummaryPage,
    },
    ConversationRecordsPage {
        request_id: String,
        source_ref: String,
        page: ConversationRecordPage,
    },
    VisualizationRead {
        request_id: String,
        source_ref: String,
        artifact: ConversationVisualizationArtifact,
    },
    ConversationUpsert {
        source_ref: String,
        conversation: RetainedConversation,
    },
    ConversationRemoved {
        source_ref: String,
        conversation_ref: String,
    },
    ConversationLiveOutput {
        source_ref: String,
        conversation_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<ConversationLiveOutput>,
    },
    ConversationLiveStatusSnapshot {
        source_ref: String,
        conversation_ref: String,
        statuses: Vec<ConversationLiveStatus>,
    },
    ConversationTurnStatusSnapshot {
        source_ref: String,
        conversation_ref: String,
        turns: Vec<ConversationTurnStatus>,
    },
    ConversationExecutionFinished {
        source_ref: String,
        conversation_ref: String,
        target_actor_ref: String,
        response: Box<ActionResponse>,
    },
    ConversationExecutionFailed {
        source_ref: String,
        conversation_ref: String,
        invocation_ref: String,
        target_actor_ref: String,
        error: String,
    },
    CommandSucceeded {
        request_id: String,
        receipt: HostCommandReceipt,
    },
    IntrinsicResponse {
        request_id: String,
        response: Box<IntrinsicCwrResponse>,
    },
    CommandFailed {
        request_id: String,
        error: String,
    },
}

include!("host/event.rs");

/// Projects a retained root conversation onto the host wire boundary.
///
/// Provider routing records and embedded adapter invocations remain in the
/// owning runtime. The returned value keeps canonical conversation identity,
/// participants, messages, and provider-neutral CWR activity.
pub fn project_conversation(conversation: &RetainedConversation) -> RetainedConversation {
    let mut projected = conversation.clone();
    projected
        .activity
        .retain_mut(|activity| match &mut activity.event {
            ConversationActivityEvent::ProviderThread { .. }
            | ConversationActivityEvent::ProviderEvent { .. }
            | ConversationActivityEvent::CommentaryProgress { .. } => false,
            ConversationActivityEvent::TurnStarted { invocation, .. } => {
                *invocation = None;
                true
            }
            _ => true,
        });
    projected
}

include!("host/validation.rs");

#[cfg(test)]
mod tests;

/// Content-free identity captured when an executable is built.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BuildIdentity {
    pub version: String,
    pub revision: Option<String>,
    pub dirty: Option<bool>,
    pub built_at: String,
    pub profile: String,
}
