//! Finite indexed public content/completion facts with current scope in one snapshot.
use super::*;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PositionCursor {
    pub position: i64,
    pub revision: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContentPageRequest {
    pub scope: Scope,
    pub conversation_ref: String,
    pub after: Option<PositionCursor>,
    pub before: Option<PositionCursor>,
    pub first: Option<usize>,
    pub last: Option<usize>,
    pub max_bytes: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContentItemFacts {
    pub position: i64,
    pub record: Value,
    pub occurred_at_unix_ms: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContentPageFacts {
    pub runtime_root_ref: String,
    pub conversation_ref: String,
    pub commit_revision: String,
    pub conversation: ResourceConversationFacts,
    pub canonical: bool,
    pub revision: String,
    pub items: Vec<ContentItemFacts>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub total_count: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompletionPageRequest {
    pub scope: Scope,
    pub conversation_ref: String,
    pub after_sequence: i64,
    pub through_sequence: Option<i64>,
    pub cursor_revision: Option<String>,
    pub first: usize,
    pub max_bytes: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CompletionPageFacts {
    pub runtime_root_ref: String,
    pub conversation_ref: String,
    pub commit_revision: String,
    pub conversation: ResourceConversationFacts,
    pub canonical: bool,
    pub revision: String,
    pub items: Vec<SummaryCompletion>,
    pub through_sequence: i64,
    pub next_after: i64,
    pub has_next_page: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PositionWindow {
    pub after: Option<PositionCursor>,
    pub before: Option<PositionCursor>,
    pub first: Option<usize>,
    pub last: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompletionWindow {
    pub after: i64,
    pub through: Option<i64>,
    pub revision: Option<String>,
    pub first: usize,
}
