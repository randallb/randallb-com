//! Canonical CWR graph persistence behind named runtime operations.
//! CWR validates protocol/domain transitions; the backend owns graph identity,
//! ownership, indexes, snapshots, conditional commit and retry receipts.
use crate::{
    invocation::{Authorizer, Context},
    CwpdbResult,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod history_read;
pub use history_read::*;
mod pages;
pub use pages::{
    CompletionPageFacts, CompletionPageRequest, ContentItemFacts, ContentPageFacts,
    ContentPageRequest, PositionCursor,
};
pub use pages::{CompletionWindow, PositionWindow};
mod resource;
pub use resource::{
    ResourceConversationFacts, ResourceExecutionFacts, ResourceFacts, ResourceRecordFacts,
};
mod native;
#[cfg(test)]
mod tests;
pub use native::{NativeConversationStore, NativeReadSnapshot, NativeReadStore};
/// Finite by-value envelope below the Workers32MiB RPC ceiling. Existing10MiB
/// retained files (20MiB hex) fit without truncation or staged partial commits.
pub const MAX_WIRE_BYTES: usize = 24 * 1024 * 1024;
pub const MAX_RECORD_BYTES: usize = MAX_WIRE_BYTES - 16384;

#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub runtime_state_version: String,
    pub workspace_binding: Option<WorkspaceBinding>,
    pub timeline_rollout: Option<String>,
}
#[derive(Debug, Clone)]
pub struct WorkspaceBinding {
    pub workspace_id: String,
    pub workspace_directory: std::path::PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub context: Context,
    pub runtime_root_ref: String,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Runtime,
    Conversation,
    Message,
    Activity,
    ConversationTransferMetadata,
    Admission,
    Invocation,
    Wait,
    File,
    VisualizationArtifact,
    ConversationTimelineConnection,
    ConversationTimelineItem,
    ToolAttempt,
    ToolOperation,
    ToolFile,
    UpstreamDeclaration,
    LocalActorIdentity,
}
impl RecordKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Conversation => "conversation",
            Self::Message => "message",
            Self::Activity => "activity",
            Self::ConversationTransferMetadata => "conversation_transfer_metadata",
            Self::Admission => "admission",
            Self::Invocation => "invocation",
            Self::Wait => "wait",
            Self::File => "file",
            Self::VisualizationArtifact => "visualization_artifact",
            Self::ConversationTimelineConnection => "conversation_timeline_connection",
            Self::ConversationTimelineItem => "conversation_timeline_item",
            Self::ToolAttempt => "tool_attempt",
            Self::ToolOperation => "tool_operation",
            Self::ToolFile => "tool_file",
            Self::UpstreamDeclaration => "upstream_declaration",
            Self::LocalActorIdentity => "local_actor_identity",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub reference: String,
    pub kind: RecordKind,
    pub schema_ref: String,
    pub properties: Value,
    pub revision: String,
}
/// A logical relationship value, including domain-defined position. Physical
/// edge identity and reverse/lookup indexes remain backend responsibilities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Relationship {
    pub reference: String,
    pub source_ref: String,
    pub kind: String,
    pub target_ref: String,
    pub position: i64,
    pub properties: Value,
    pub revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "selection", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    /// Runtime/admission/invocation/timeline metadata and conversation headers;
    /// closed message/activity bodies remain cold and are read separately.
    Bootstrap,
    Conversation {
        conversation_ref: String,
    },
    Records {
        kind: RecordKind,
    },
    /// Headers, first message and fork lineage; closed history stays cold.
    ConversationMetadata {
        conversation_ref: Option<String>,
    },
    /// Canonical provider-item deduplication lookup, before staging publication.
    Commentary {
        provider_integration_ref: String,
        response_or_turn_id: String,
        item_id: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    pub scope: Scope,
    pub selection: Selection,
    pub continuation: Option<String>,
    pub page_size: usize,
    pub max_bytes: usize,
    pub expected_revision: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SnapshotPage {
    pub revision: String,
    pub metadata: Value,
    pub records: Vec<Record>,
    pub relationships: Vec<Relationship>,
    pub continuation: Option<String>,
    pub complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WriteRecord {
    pub reference: String,
    pub kind: RecordKind,
    pub properties: Value,
    pub owner_ref: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RemoveRecord {
    pub reference: String,
    pub kind: RecordKind,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MembershipKind {
    Conversation,
    Parent,
    Participant,
    Archived,
    Message,
    Activity,
    TransferMetadata,
    Admission,
    Wait,
    Execution,
    ParticipantVersion,
    ConversationTimelineRoot,
    ConversationTimelineItem,
    ConversationTimelineChild,
    ConversationTimelineMessage,
    ConversationTimelineInvocation,
    ToolAttempt,
    ToolOperation,
    ToolFile,
    ChildInvocation,
    UpstreamDeclaration,
}
impl MembershipKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Parent => "parent",
            Self::Participant => "participant",
            Self::Archived => "archived",
            Self::Message => "message",
            Self::Activity => "activity",
            Self::TransferMetadata => "transfer_metadata",
            Self::Admission => "admission",
            Self::Wait => "wait",
            Self::Execution => "execution",
            Self::ParticipantVersion => "participant_version",
            Self::ConversationTimelineRoot => "conversation_timeline_root",
            Self::ConversationTimelineItem => "conversation_timeline_item",
            Self::ConversationTimelineChild => "conversation_timeline_child",
            Self::ConversationTimelineMessage => "conversation_timeline_message",
            Self::ConversationTimelineInvocation => "conversation_timeline_invocation",
            Self::ToolAttempt => "tool_attempt",
            Self::ToolOperation => "tool_operation",
            Self::ToolFile => "tool_file",
            Self::ChildInvocation => "child_invocation",
            Self::UpstreamDeclaration => "upstream_declaration",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub target_ref: String,
    pub position: i64,
    pub properties: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Membership {
    pub source_ref: String,
    pub kind: MembershipKind,
    pub members: Vec<Member>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum MembershipChange {
    Upsert {
        source_ref: String,
        kind: MembershipKind,
        member: Member,
    },
    Remove {
        source_ref: String,
        kind: MembershipKind,
        target_ref: String,
    },
    Replace {
        membership: Membership,
    },
}
/// The owning CWR publication logic selects visible semantic items. cwpdb owns
/// their materialized lookup records and index maintenance. Replace only the
/// changed suffix rather than retransmitting every growing history prefix.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PublishedItem {
    Message {
        message_ref: String,
        position: u64,
    },
    Turn {
        invocation_ref: String,
        position: u64,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PublishedActivity {
    pub activity_ref: String,
    pub position: u64,
    pub record: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PublicationSuffix {
    pub conversation_ref: String,
    pub from_position: u64,
    pub items: Vec<PublishedItem>,
    /// Changed projected activity records; independent of semantic anchors.
    #[serde(default)]
    pub activities: Vec<PublishedActivity>,
    #[serde(default)]
    pub remove_activities: Vec<String>,
}

/// One CWR domain mutation. Only changed canonical records and authoritative
/// membership facts cross this boundary; never a repeated whole-history blob.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Changes {
    /// Each item is one finite semantic operation, expanded atomically by cwpdb.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timeline_items: Vec<TimelineItemWrite>,
    pub records: Vec<WriteRecord>,
    pub remove_records: Vec<RemoveRecord>,
    pub memberships: Vec<MembershipChange>,
    pub publications: Vec<PublicationSuffix>,
    pub state_version: Option<String>,
    pub streaming_runs: Option<Value>,
    #[serde(default)]
    pub timeline_rollout: Option<String>,
    /// Opaque validated CWR wrapper; null fields remain protocol data.
    #[serde(default)]
    pub context_policy: Option<Value>,
    #[serde(default)]
    pub repair_legacy: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JournalCommit {
    pub request: crate::invocation::Mutation,
    pub now_unix_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthorityGuard {
    pub reference: String,
    pub expected_revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
    pub scope: Scope,
    pub request_id: String,
    pub expected_revision: String,
    pub changes: Changes,
    #[serde(default)]
    pub journal: Option<JournalCommit>,
    #[serde(default)]
    pub authority_guards: Vec<AuthorityGuard>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CommitOutcome {
    Committed {
        revision: String,
        commit_revision: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        journal_receipt: Option<crate::invocation::Receipt>,
    },
    CommittedResultExpired,
}
/// Every call rechecks trusted domain authority. Pages are pinned to the first
/// returned revision; a concurrent change returns revision_conflict instead of a
/// mixed bootstrap state. An invalid/expired continuation restarts the read.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy)]
pub enum PublicationAudit {
    Start,
    Drain,
}
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PublicationWriteCounts {
    pub message: u64,
    pub anchor: u64,
    pub activity: u64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointKinds {
    pub node_kind: Option<String>,
    pub edge_kind: Option<String>,
}

pub trait ConversationStorage: Send {
    fn history_read(
        &mut self,
        _request: &HistoryReadRequest,
        _control: &ReadControl,
        _authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<HistoryReadFacts>> {
        Err("finite history read unavailable".into())
    }

    fn read_selection(
        &mut self,
        _request: &ReadSelectionRequest,
        _control: &ReadControl,
        _authorizer: &dyn Authorizer,
    ) -> CwpdbResult<ReadSelectionFacts> {
        Err("finite public read selection is unavailable for this backend".into())
    }
    fn timeline_page(
        &mut self,
        _request: &TimelineReadRequest,
        _control: &ReadControl,
        _authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<TimelineReadFacts>> {
        Err("finite timeline page unavailable".into())
    }

    fn content_page(
        &mut self,
        _request: &ContentPageRequest,
        _authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<ContentPageFacts>> {
        Err("finite conversation content page is unavailable for this backend".into())
    }
    fn completion_page(
        &mut self,
        _request: &CompletionPageRequest,
        _authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<CompletionPageFacts>> {
        Err("finite conversation completion page is unavailable for this backend".into())
    }

    fn resource(
        &mut self,
        _scope: &Scope,
        _reference: &str,
        _authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<ResourceFacts>> {
        Err("exact conversation resource is unavailable for this backend".into())
    }

    fn history_status(
        &mut self,
        _scope: &Scope,
        _conversation_ref: &str,
        _authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<HistoryStatus>> {
        Err("exact conversation history status is unavailable for this backend".into())
    }
    fn contains_member(
        &mut self,
        _scope: &Scope,
        _source: &str,
        _kind: MembershipKind,
        _target: &str,
        _authorizer: &dyn Authorizer,
    ) -> CwpdbResult<bool> {
        Err("exact conversation membership lookup is unavailable for this backend".into())
    }

    /// Current exact endpoint types. Retained identity claims alone are not records.
    fn endpoint_kinds(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<EndpointKinds>;

    /// Exact current summary facts, after current trusted authorization. No history mirror.
    fn summary(
        &mut self,
        _scope: &Scope,
        _reference: &str,
        _authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<SummaryFacts>> {
        Err("exact conversation summary is unavailable for this backend".into())
    }

    #[cfg(feature = "test-support")]
    fn audit_publication_writes(
        &mut self,
        _action: PublicationAudit,
    ) -> CwpdbResult<PublicationWriteCounts> {
        Err("publication write audit is unavailable for this backend".into())
    }
    /// Whether a retained node or relationship already occupies this identity.
    /// A reference binding alone is not a retained record.
    fn identity_in_use(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<bool>;
    /// Backend-owned retained ordering token, usable only for domain ordering.
    fn retained_order(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<i64>>;
    fn published_progress_conversation(
        &mut self,
        scope: &Scope,
        source_activity_ref: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<String>>;
    fn read(
        &mut self,
        request: &ReadRequest,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<SnapshotPage>;
    fn get(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<Record>>;
    fn commit(
        &mut self,
        request: &Mutation,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<CommitOutcome>;
    fn reconcile(
        &mut self,
        request: &Mutation,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<CommitOutcome>>;
}

mod summary_page;
pub use summary_page::{
    SummaryWindowFacts, SummaryWindowKey, SummaryWindowRequest, SummaryWindowRow,
};
mod summary;
pub use summary::{SummaryCompletion, SummaryFacts};

mod history;
pub use history::HistoryStatus;

mod timeline;
pub use timeline::{TimelineContent, TimelineItemWrite};

/// A source document changed before the atomic publication commit.
#[derive(Debug)]
pub struct SourceAuthorityConflict;
impl std::fmt::Display for SourceAuthorityConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("source authority revision conflict")
    }
}
impl std::error::Error for SourceAuthorityConflict {}

mod read_control;
pub use read_control::{ReadCancellation, ReadControl};
mod timeline_read;
pub use timeline_read::{TimelineAnchorFacts, TimelineReadFacts, TimelineReadRequest};

mod inspector_read;
pub use inspector_read::InvocationStopFacts;
pub use inspector_read::{
    InspectorCollection, InspectorEntryFacts, InspectorKey, InspectorReadRequest,
    InspectorTextFacts, InspectorTextField, InspectorTextRequest,
};

mod read_selection;
pub use read_selection::{ReadSelectionFacts, ReadSelectionOperation, ReadSelectionRequest};

mod read_snapshot;
pub use read_snapshot::{ReadSelectionSource, ReadSnapshot, RemoteReadSnapshot};
