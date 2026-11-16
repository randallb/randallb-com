//! Finite summary windows and their frozen presentation facts.
use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryWindowKey {
    pub creation_order: i64,
    pub conversation_ref: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryWindowRow {
    pub canonical_preview: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_work: Vec<SummaryCompletion>,
    pub key: SummaryWindowKey,
    pub title: Option<String>,
    pub archived: bool,
    pub last_message_ref: Option<String>,
    pub last_message_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_work_refs: Vec<String>,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryWindowFacts {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub membership_revision: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub revision: String,
    pub summaries: Vec<SummaryWindowRow>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub total_count: usize,
    pub archived_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryWindowRequest {
    pub participant_actor_ref: Option<String>,
    pub archived: Option<bool>,
    pub owner_context_filter: Option<(String, bool)>,
    pub after: Option<SummaryWindowKey>,
    pub before: Option<SummaryWindowKey>,
    pub first: Option<usize>,
    pub last: Option<usize>,
}

#[derive(Clone, Copy)]
pub(super) struct SummaryWindow<'a> {
    pub after: Option<&'a SummaryWindowKey>,
    pub before: Option<&'a SummaryWindowKey>,
    pub first: Option<usize>,
    pub last: Option<usize>,
}
