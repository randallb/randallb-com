//! Finite exact startup-recovery facts; CWR owns repair policy.
use super::*;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent retained-data predicates preserve the finite wire contract."
)]
pub struct HistoryStatus {
    pub has_timeline_root: bool,
    pub has_streaming: bool,
    pub has_legacy_context: bool,
    pub terminal_activity: bool,
    pub revision: String,
}
