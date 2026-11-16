//! Finite published-history facts. CWR owns delivery projection and excerpts.
use super::*;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistorySelection {
    Page {
        after: Option<PositionCursor>,
        before: Option<PositionCursor>,
        first: Option<usize>,
        last: Option<usize>,
    },
    ArchivePage {
        after: Option<PositionCursor>,
        before: Option<PositionCursor>,
        first: Option<usize>,
        last: Option<usize>,
    },
    PagePositions {
        after: Option<PositionCursor>,
        before: Option<PositionCursor>,
        first: Option<usize>,
        last: Option<usize>,
    },
    ScanPositions {
        after_position: Option<i64>,
        after_revision: Option<String>,
        first: usize,
    },
    RetainedMessage {
        message_ref: String,
    },
    ArchiveMessage {
        message_ref: String,
    },
    ContentPositions {
        after: Option<PositionCursor>,
        before: Option<PositionCursor>,
        first: Option<usize>,
        last: Option<usize>,
    },
    Message {
        message_ref: String,
    },
    Replies {
        message_ref: String,
        after_message_ref: Option<String>,
        after_revision: Option<String>,
        first: usize,
    },
    Scan {
        after_position: Option<i64>,
        after_revision: Option<String>,
        first: usize,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HistoryReadRequest {
    pub scope: Scope,
    pub conversation_ref: String,
    pub selection: HistorySelection,
    pub expected_commit_revision: Option<String>,
    pub participant_actor_ref: Option<String>,
    pub include_execution: bool,
    pub deadline_unix_ms: i64,
    pub max_bytes: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryItemFacts {
    pub position: i64,
    pub message_ref: String,
    pub record: Option<Value>,
    pub payload_bytes: usize,
    pub origin_invocation_ref: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HistoryReadFacts {
    pub runtime_root_ref: String,
    pub conversation_ref: String,
    pub commit_revision: String,
    pub conversation: ResourceConversationFacts,
    pub canonical: bool,
    pub revision: String,
    pub items: Vec<HistoryItemFacts>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub total_count: u64,
    pub reply_message_refs: Vec<String>,
}
pub const MAX_HISTORY_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
