//! One finite visible timeline window and its rendering dependencies.
use super::*;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TimelineReadRequest {
    pub scope: Scope,
    pub conversation_ref: String,
    pub after: Option<PositionCursor>,
    pub before: Option<PositionCursor>,
    pub first: Option<usize>,
    pub last: Option<usize>,
    pub participant_actor_ref: Option<String>,
    pub include_execution: bool,
    pub deadline_unix_ms: i64,
    pub max_bytes: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineAnchorFacts {
    Message {
        position: i64,
        message_ref: String,
    },
    Turn {
        position: i64,
        invocation_ref: String,
    },
}
impl TimelineAnchorFacts {
    pub fn position(&self) -> i64 {
        match self {
            Self::Message { position, .. } | Self::Turn { position, .. } => *position,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TimelineReadFacts {
    pub runtime_root_ref: String,
    pub conversation_ref: String,
    pub commit_revision: String,
    pub conversation: ResourceConversationFacts,
    pub revision: String,
    pub anchors: Vec<TimelineAnchorFacts>,
    pub messages: Vec<Value>,
    pub activity: Vec<Value>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub total_count: u64,
}
