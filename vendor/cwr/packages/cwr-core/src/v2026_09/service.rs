//! Process-neutral protocol for one conversation-owning workspace runtime.

use serde::{Deserialize, Serialize};

use crate::intrinsic_cwr::{IntrinsicCwrCommand, IntrinsicCwrResponse};

use super::conversation::{
    ConversationAdmissionRecord, ConversationIngress, ConversationMessage, ConversationSummaryPage,
    ConversationSummaryPageRequest, ConversationVisualizationArtifact, HostConversationExchange,
    RetainedConversation,
};
use super::host::{
    ConversationLiveOutput, ConversationLiveStatus, ConversationLiveStatusName,
    ConversationTurnStatus, HostWorkspace,
};
use super::invocation::ActionResponse;
use super::local::LocalCatalog;

pub const RUNTIME_SERVICE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CommunicationDestination {
    pub destination_ref: String,
    pub display_name: String,
    #[serde(default)]
    pub default_for_new_conversations: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GatewayInvocationKind {
    Request,
    Ingress,
    Delivery,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GatewayInvocationTerminal {
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeServiceCommand {
    /// Connection-owned development replacement; ordinary drains remain independent.
    DevGenerationRestart {
        version: u32,
        request_id: String,
        operation: String,
        generation: String,
    },
    /// Register a known operation for zero-work normal shutdown finalization.
    ArmConfiguredResident {
        version: u32,
        request_id: String,
        nonce: String,
        expected_runtime_ref: String,
        expected_owner_pid: i32,
    },
    /// One atomic retirement attempt; busy work rejects without committing.
    RetireConfiguredResident {
        version: u32,
        request_id: String,
        nonce: String,
        expected_runtime_ref: String,
        expected_owner_pid: i32,
    },
    Upstream {
        version: u32,
        request_id: String,
        host_actor_ref: String,
        command: super::upstream::UpstreamCommand,
    },
    Drain {
        version: u32,
        request_id: String,
        operation: String,
    },
    ExitWhenIdle {
        version: u32,
        request_id: String,
    },
    Snapshot {
        version: u32,
        request_id: String,
        #[serde(default)]
        metadata_only: bool,
    },
    ReadRootConversations {
        version: u32,
        request_id: String,
        page: ConversationSummaryPageRequest,
        timeout_ms: u64,
    },
    ReadConversationRecords {
        version: u32,
        request_id: String,
        read: super::conversation::ConversationRecordReadRequest,
        timeout_ms: u64,
    },
    ReadVisualization {
        version: u32,
        request_id: String,
        artifact_ref: String,
    },
    CancelRead {
        version: u32,
        request_id: String,
        target_request_id: String,
    },
    InspectActor {
        version: u32,
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor_ref: Option<String>,
    },
    ReplyPersonRequest {
        version: u32,
        request_id: String,
        actor_ref: String,
        conversation_ref: String,
        invocation_ref: String,
        text: String,
    },
    SubmitText {
        version: u32,
        request_id: String,
        integration_ref: String,
        author_actor_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_ref: Option<String>,
        text: String,
    },
    ReplayText {
        version: u32,
        request_id: String,
        integration_ref: String,
        author_actor_ref: String,
        conversation_ref: String,
        text: String,
    },
    SubmitAction {
        version: u32,
        request_id: String,
        integration_ref: String,
        author_actor_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_ref: Option<String>,
        activation: String,
        input: serde_json::Value,
    },
    /// Bind a configured gateway to this service connection. The host fixes the
    /// integration ref used by subsequent SubmitAction commands on this writer.
    RegisterGateway {
        version: u32,
        request_id: String,
        connection_ref: String,
        integration_ref: String,
        approval_fingerprint: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ingress_action_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery_action_name: Option<String>,
    },
    InspectGatewayInvocation {
        version: u32,
        request_id: String,
        connection_ref: String,
        invocation_ref: String,
        invocation_kind: GatewayInvocationKind,
        input: serde_json::Value,
    },
    RecordHostExchange {
        version: u32,
        request_id: String,
        requesting_actor_ref: String,
        host_actor_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_ref: Option<String>,
        request_text: String,
        response_text: String,
    },
    AdmitIngress {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_actor_ref: Option<String>,
        version: u32,
        request_id: String,
        ingress: Box<ConversationIngress>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor_integration_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachment: Option<RuntimeIngressAttachment>,
    },
    StopConversation {
        version: u32,
        request_id: String,
        conversation_ref: String,
        requesting_actor_ref: String,
        /// An exact queued or running turn selected by the host's control.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation_ref: Option<String>,
        /// When a provider's stop control was clicked, not when it was delivered.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_at_unix_ms: Option<u64>,
    },
    SetConversationLiveStatus {
        version: u32,
        request_id: String,
        conversation_ref: String,
        subject_actor_ref: String,
        publisher_session_id: String,
        sequence: u64,
        target_actor_refs: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<ConversationLiveStatusName>,
        lease_ms: u64,
    },
    ExecuteIntrinsic {
        version: u32,
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_id: Option<String>,
        command: IntrinsicCwrCommand,
        requesting_actor_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_ref: Option<String>,
    },
    ReloadGraph {
        version: u32,
        request_id: String,
        requesting_actor_ref: String,
    },
    RegisterCommunicationAdapter {
        version: u32,
        request_id: String,
        adapter_session_ref: String,
        integration_ref: String,
        destinations: Vec<CommunicationDestination>,
    },
    AttachmentCreated {
        version: u32,
        request_id: String,
        attachment_ref: String,
    },
    DeliverySucceeded {
        version: u32,
        request_id: String,
        attachment_ref: String,
        delivery_ref: String,
    },
    AttachmentFailed {
        version: u32,
        request_id: String,
        attachment_ref: String,
        error: String,
    },
    DeliveryFailed {
        /// Explicit opt-out from retry; diagnostic text never controls permanence.
        #[serde(default)]
        permanent: bool,
        /// Optional Unix millisecond timestamp for the next retry.
        #[serde(default)]
        retry_after: Option<u64>,
        version: u32,
        request_id: String,
        attachment_ref: String,
        delivery_ref: String,
        error: String,
    },
}

impl RuntimeServiceCommand {
    pub fn request_id(&self) -> &str {
        match self {
            Self::DevGenerationRestart { request_id, .. }
            | Self::RetireConfiguredResident { request_id, .. }
            | Self::ArmConfiguredResident { request_id, .. }
            | Self::Upstream { request_id, .. }
            | Self::Drain { request_id, .. }
            | Self::ExitWhenIdle { request_id, .. }
            | Self::Snapshot { request_id, .. }
            | Self::ReadRootConversations { request_id, .. }
            | Self::ReadConversationRecords { request_id, .. }
            | Self::ReadVisualization { request_id, .. }
            | Self::CancelRead { request_id, .. }
            | Self::InspectActor { request_id, .. }
            | Self::ReplyPersonRequest { request_id, .. }
            | Self::SubmitText { request_id, .. }
            | Self::ReplayText { request_id, .. }
            | Self::SubmitAction { request_id, .. }
            | Self::RegisterGateway { request_id, .. }
            | Self::InspectGatewayInvocation { request_id, .. }
            | Self::RecordHostExchange { request_id, .. }
            | Self::AdmitIngress { request_id, .. }
            | Self::StopConversation { request_id, .. }
            | Self::SetConversationLiveStatus { request_id, .. }
            | Self::ExecuteIntrinsic { request_id, .. }
            | Self::ReloadGraph { request_id, .. }
            | Self::RegisterCommunicationAdapter { request_id, .. }
            | Self::AttachmentCreated { request_id, .. }
            | Self::DeliverySucceeded { request_id, .. }
            | Self::AttachmentFailed { request_id, .. }
            | Self::DeliveryFailed { request_id, .. } => request_id,
        }
    }

    pub fn version(&self) -> u32 {
        match self {
            Self::DevGenerationRestart { version, .. }
            | Self::RetireConfiguredResident { version, .. }
            | Self::ArmConfiguredResident { version, .. }
            | Self::Upstream { version, .. }
            | Self::Drain { version, .. }
            | Self::ExitWhenIdle { version, .. }
            | Self::Snapshot { version, .. }
            | Self::ReadRootConversations { version, .. }
            | Self::ReadConversationRecords { version, .. }
            | Self::ReadVisualization { version, .. }
            | Self::CancelRead { version, .. }
            | Self::InspectActor { version, .. }
            | Self::ReplyPersonRequest { version, .. }
            | Self::SubmitText { version, .. }
            | Self::ReplayText { version, .. }
            | Self::SubmitAction { version, .. }
            | Self::RegisterGateway { version, .. }
            | Self::InspectGatewayInvocation { version, .. }
            | Self::RecordHostExchange { version, .. }
            | Self::AdmitIngress { version, .. }
            | Self::StopConversation { version, .. }
            | Self::SetConversationLiveStatus { version, .. }
            | Self::ExecuteIntrinsic { version, .. }
            | Self::ReloadGraph { version, .. }
            | Self::RegisterCommunicationAdapter { version, .. }
            | Self::AttachmentCreated { version, .. }
            | Self::DeliverySucceeded { version, .. }
            | Self::AttachmentFailed { version, .. }
            | Self::DeliveryFailed { version, .. } => *version,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeAdmissionReceipt {
    pub record: ConversationAdmissionRecord,
    pub conversation: RetainedConversation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeIngressAttachment {
    pub integration_ref: String,
    pub destination_ref: String,
    pub binding_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeAttachmentRequest {
    pub attachment_ref: String,
    pub conversation_ref: String,
    pub integration_ref: String,
    pub destination_ref: String,
    pub root_message: ConversationMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDeliveryRequest {
    pub attachment_ref: String,
    pub delivery_ref: String,
    pub conversation_ref: String,
    /// The root invocation whose request this message answers, if any.
    /// For published progress, its own origin remains in `message.progress`.
    /// Communication adapters use this root identity to correlate delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_ref: Option<String>,
    pub message: ConversationMessage,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub explicit_recipient_actor_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeServiceEvent {
    /// Active work has drained; the next connection must use this generation.
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
    DrainStatus {
        request_id: String,
        remaining: usize,
    },
    BuildIdentity {
        request_id: String,
        identity: super::host::BuildIdentity,
    },
    Ready {
        runtime_ref: String,
    },
    CatalogUpdated {
        catalog: LocalCatalog,
    },
    ActorInspected {
        request_id: String,
        workspace: HostWorkspace,
        catalog: LocalCatalog,
    },
    ConversationSnapshot {
        conversations: Vec<RetainedConversation>,
    },
    RootConversationsPage {
        request_id: String,
        page: ConversationSummaryPage,
    },
    ConversationRecordsPage {
        request_id: String,
        page: super::conversation::ConversationRecordPage,
    },
    VisualizationRead {
        request_id: String,
        artifact: ConversationVisualizationArtifact,
    },
    ConversationUpsert {
        conversation: RetainedConversation,
    },
    ConversationLiveOutput {
        conversation_ref: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<ConversationLiveOutput>,
    },
    ConversationLiveStatusSnapshot {
        conversation_ref: String,
        statuses: Vec<ConversationLiveStatus>,
    },
    ConversationTurnStatusSnapshot {
        conversation_ref: String,
        turns: Vec<ConversationTurnStatus>,
    },
    ConversationExecutionFinished {
        conversation_ref: String,
        target_actor_ref: String,
        response: Box<ActionResponse>,
    },
    ConversationExecutionFailed {
        conversation_ref: String,
        invocation_ref: String,
        target_actor_ref: String,
        error: String,
    },
    CommandSucceeded {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        admission: Option<Box<RuntimeAdmissionReceipt>>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        stopped_invocation_refs: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intrinsic_response: Option<Box<IntrinsicCwrResponse>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_exchange: Option<Box<HostConversationExchange>>,
    },
    CommandFailed {
        request_id: String,
        error: String,
    },
    GatewayInvocationInspected {
        request_id: String,
        authorized: bool,
        /// Retained result for an exact invocation, including after reconnect.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal: Option<GatewayInvocationTerminal>,
    },
    CreateAttachment {
        request: RuntimeAttachmentRequest,
    },
    DeliverMessage {
        request: RuntimeDeliveryRequest,
    },
}

#[cfg(test)]
mod tests;
