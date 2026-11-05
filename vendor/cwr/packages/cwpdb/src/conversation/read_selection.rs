//! Declared finite reads for one coherent public selection. Remote continuations
//! pin the first commit and refuse changes instead of returning mixed facts.
use super::*;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadSelectionRequest {
    pub scope: Scope,
    pub expected_commit_revision: Option<String>,
    pub selection: ReadSelectionOperation,
    pub deadline_unix_ms: i64,
    pub max_bytes: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadSelectionFacts {
    pub runtime_root_ref: String,
    pub commit_revision: String,
    /// The finite operation determines the public result shape. No stored graph
    /// selector, SQL text, physical identifier or callback is accepted.
    pub result: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadSelectionOperation {
    Position,
    ContentMessage {
        conversation_ref: String,
        message_ref: String,
        include_execution: bool,
    },
    ContentPositions {
        conversation_ref: String,
        window: PositionWindow,
        include_execution: bool,
    },
    CompletionPage {
        conversation_ref: String,
        window: CompletionWindow,
        include_execution: bool,
    },
    TimelinePage {
        conversation_ref: String,
        window: PositionWindow,
        include_execution: bool,
    },
    LegacyTimelinePage {
        conversation_ref: String,
        window: PositionWindow,
        include_execution: bool,
    },
    LegacyTimelineMessage {
        conversation_ref: String,
        key: PositionCursor,
        message_ref: String,
        include_execution: bool,
    },

    SummaryWindow {
        request: SummaryWindowRequest,
    },
    SummaryRow {
        reference: String,
        workspace: Option<(Option<String>, String)>,
    },
    SummaryMessageRecord {
        conversation_ref: String,
        message_ref: String,
    },
    HistoryRead {
        conversation_ref: String,
        selection: HistorySelection,
        participant_actor_ref: Option<String>,
        include_execution: bool,
    },
    Current {
        conversation_ref: String,
    },
    Invocation {
        conversation_ref: String,
        invocation_ref: String,
    },
    Resource {
        reference: String,
    },
    ConversationExists {
        conversation_ref: String,
        include_execution: bool,
        owner: Option<(String, bool)>,
    },
    ParticipantVisible {
        conversation_ref: String,
        actor_ref: String,
    },
    RootConversationPresentation {
        conversation_ref: String,
    },
    ForkSource {
        conversation_ref: String,
    },
    OwnerContext {
        conversation_ref: String,
    },
    TimelineItemConversation {
        item_ref: String,
    },
    ActorIdentity {
        actor_ref: String,
    },
    ActorCandidate {
        actor_ref: String,
    },
    ActorConversations {
        actor_ref: String,
    },
    ActorResources {
        actor_ref: String,
    },
    ArtifactExists {
        reference: String,
    },
    ArtifactMessages {
        reference: String,
    },
    ArtifactHtml {
        reference: String,
    },
    InvocationReplyRef {
        invocation_ref: String,
    },
    InvocationForkDestination {
        invocation_ref: String,
        intrinsic_actor_ref: String,
    },
    InvocationStopFacts {
        invocation_ref: String,
        intrinsic_actor_ref: String,
    },
    InvocationVisible {
        root_ref: String,
        invocation_ref: String,
    },
    StreamingMessageMetadata {
        message_ref: String,
    },
    MessageActionInvocation {
        conversation_ref: String,
        message_ref: String,
    },
    ProgressActivity {
        invocation_ref: String,
        source_activity_ref: String,
    },
    ProgressInLatestAttempt {
        invocation_ref: String,
        sequence: Option<u64>,
    },
    LatestWorkActivity {
        invocation_ref: String,
        activity_ref: String,
    },
    TimelineRoot {
        conversation_ref: String,
        include_execution: bool,
    },
    TimelineConnectionMetadata {
        conversation_ref: String,
        root_ref: String,
        connection_ref: String,
    },
    TimelineCursorExists {
        connection_ref: String,
        sequence: i64,
        item_ref: String,
    },
    TimelineConnectionItems {
        connection_ref: String,
        lower: Option<i64>,
        upper: Option<i64>,
        limit: usize,
        backwards: bool,
    },
    TimelineConnectionCount {
        conversation_ref: String,
        connection_ref: String,
    },
    TimelineItem {
        conversation_ref: String,
        item_ref: String,
    },
    InspectorCaptureAvailable {
        invocation_ref: String,
    },
    InspectorDetailsAllowed {
        invocation_ref: String,
        actor_ref: String,
    },
    InspectorInvocationRoot {
        invocation_ref: String,
    },
    InspectorScope {
        actor_ref: String,
        invocation_ref: String,
        collection: InspectorCollection,
        activity_ref: Option<String>,
    },
    InspectorPage {
        request: InspectorReadRequest,
    },
    InspectorActivitySequence {
        invocation_ref: String,
        conversation_ref: String,
        activity_ref: String,
    },
    InspectorExitCode {
        invocation_ref: String,
        conversation_ref: String,
        actor_ref: String,
        activity_ref: String,
    },
    InspectorText {
        request: InspectorTextRequest,
    },
    InvocationTargetSource {
        invocation_ref: String,
    },
    MessageIsCommentary {
        message_ref: String,
        invocation_ref: String,
    },
    MessageIsFinalReply {
        message_ref: String,
        target_actor_ref: String,
        source_message_ref: Option<String>,
    },
    LastAddressedActors {
        conversation_ref: String,
        root_ref: String,
        author_actor_ref: String,
    },
}

mod authority;
