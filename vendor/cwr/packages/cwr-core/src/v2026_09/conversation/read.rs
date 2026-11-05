use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::sync::{Arc, Mutex};

use super::{invalid, invalid_error};
use crate::CwrResult;

#[cfg(any(test, feature = "test-read-metrics"))]
pub(crate) mod step_measurement;
#[cfg(feature = "test-read-metrics")]
pub use step_measurement::measure_path_reads;

mod selection;
pub(crate) use selection::ReadStoreLocation;

mod publication;
pub use publication::*;

mod current;
pub use current::*;

mod indexed;
pub(crate) use indexed::HistoryProjection;
pub use indexed::{
    ConversationBrowseEntry, ConversationBrowsePage, ConversationCompletionPage,
    ConversationContent, ConversationContentPage, ConversationHistoryEntry,
    ConversationHistoryPage, ConversationLegacyTimelinePage, ConversationMessageLinks,
    ConversationPositionKey, ConversationRecordPage, ConversationRecordPageRequest,
    ConversationRecordReadRequest, ConversationRecordWindow, ConversationSearchPage,
    ConversationTimelineAnchor, ConversationTimelinePage, MAX_CONVERSATION_ACTION_EXCERPT_BYTES,
    MAX_CONVERSATION_BROWSE_PAGE_SIZE, MAX_CONVERSATION_BROWSE_RESPONSE_BYTES,
    MAX_CONVERSATION_EXCERPT_BYTES, MAX_CONVERSATION_RECORD_PAGE_SIZE,
    MAX_CONVERSATION_SEARCH_QUERY_CHARACTERS,
};

include!("read/summaries.rs");

pub const MAX_CONVERSATION_SUMMARY_PAGE_SIZE: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationSummaryKey {
    pub creation_order: i64,
    pub conversation_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationSummary {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_work: Vec<CompletedWorkSummary>,
    pub key: ConversationSummaryKey,
    pub title: Option<String>,
    pub archived: bool,
    pub last_message_ref: Option<String>,
    pub last_message_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_work_refs: Vec<String>,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletedWorkSummary {
    pub invocation_ref: String,
    pub sequence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationSummaryPage {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub membership_revision: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub revision: String,
    pub summaries: Vec<ConversationSummary>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub total_count: usize,
    pub archived_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationSummaryPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_context_ref: Option<String>,
    #[serde(default)]
    pub include_legacy_owner: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ConversationSummaryKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ConversationSummaryKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<usize>,
}

impl ConversationSummaryPageRequest {
    pub(crate) fn owner_filter(&self) -> Option<(String, bool)> {
        match self.owner_context_ref.as_ref() {
            Some(owner) => Some((owner.clone(), self.include_legacy_owner)),
            // Empty is an internal legacy-only sentinel. Conversation admission
            // rejects empty owner refs, and scope_clause emits IS NULL exactly.
            None if self.include_legacy_owner => Some((String::new(), true)),
            None => None,
        }
    }

    pub fn window(&self) -> ConversationSummaryWindow<'_> {
        ConversationSummaryWindow {
            after: self.after.as_ref(),
            before: self.before.as_ref(),
            first: self.first,
            last: self.last,
        }
    }

    pub fn validate(&self) -> CwrResult<()> {
        validate_window(self.window())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ConversationSummaryWindow<'key> {
    pub after: Option<&'key ConversationSummaryKey>,
    pub before: Option<&'key ConversationSummaryKey>,
    pub first: Option<usize>,
    pub last: Option<usize>,
}

#[derive(Clone)]
pub struct ConversationReadCancellation {
    storage: cwpdb_store::conversation::ReadCancellation,
}
impl ConversationReadCancellation {
    pub(crate) fn pending() -> Self {
        Self {
            storage: Default::default(),
        }
    }
    pub fn cancel(&self) {
        self.storage.cancel();
    }
    pub(crate) fn storage_control(
        &self,
        deadline: Instant,
    ) -> cwpdb_store::conversation::ReadControl {
        cwpdb_store::conversation::ReadControl {
            cancellation: self.storage.clone(),
            deadline,
            vm_steps: None,
        }
    }
    pub(crate) fn storage_control_for_path(
        &self,
        deadline: Instant,
        path: &Path,
    ) -> cwpdb_store::conversation::ReadControl {
        let control = self.storage_control(deadline);
        #[cfg(any(test, feature = "test-read-metrics"))]
        let control = {
            let mut observed = control;
            observed.vm_steps = step_measurement::current_for_path(path);
            observed
        };
        #[cfg(not(any(test, feature = "test-read-metrics")))]
        let _ = path;
        control
    }
    pub(crate) fn ensure_active(&self, deadline: Instant) -> CwrResult<()> {
        self.storage_control(deadline).check()
    }
}

// Optional resource lookups also receive owner IDs and unknown URL text. Only
// canonical CWR references can name retained conversation records.
fn canonical_lookup_reference(reference: &str) -> bool {
    reference.starts_with("cwp://")
        && reference.len() <= 4096
        && !reference.contains('#')
        && crate::CwrRef::parse(reference).is_ok()
}

/// One storage-owned read selection; CWR retains protocol presentation and visibility policy.
pub struct ConversationReadStore {
    path: PathBuf,
    factory: selection::ReadFactory,
}
pub struct ConversationReadOperation<'store> {
    include_execution_conversations: bool,
    owner_context_filter: Option<(String, bool)>,
    path: &'store Path,
    deadline: Instant,
    cancellation: ConversationReadCancellation,
    finite: FiniteSnapshot<'store>,
}
enum FiniteSnapshot<'store> {
    Owned(Box<cwpdb_store::conversation::ReadSnapshot>),
    Borrowed(&'store cwpdb_store::conversation::ReadSnapshot),
}
impl std::ops::Deref for FiniteSnapshot<'_> {
    type Target = cwpdb_store::conversation::ReadSnapshot;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(snapshot) => snapshot,
            Self::Borrowed(snapshot) => snapshot,
        }
    }
}
impl ConversationReadStore {
    pub fn open(path: impl AsRef<Path>, runtime_root_ref: &str) -> CwrResult<Self> {
        let cancellation = ConversationReadCancellation::pending();
        Self::open_until(
            path,
            runtime_root_ref,
            Instant::now() + Duration::from_secs(2),
            &cancellation,
        )
    }
    pub(crate) fn open_until(
        path: impl AsRef<Path>,
        runtime_root_ref: &str,
        deadline: Instant,
        cancellation: &ConversationReadCancellation,
    ) -> CwrResult<Self> {
        let path = path.as_ref().to_owned();
        let store = cwpdb_store::conversation::NativeReadStore::open(
            &path,
            runtime_root_ref,
            &cancellation.storage_control_for_path(deadline, &path),
        )?;
        Ok(Self {
            path,
            factory: selection::ReadFactory::Native(store),
        })
    }
    pub fn begin_operation(
        &mut self,
        timeout: Duration,
    ) -> CwrResult<ConversationReadOperation<'_>> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| invalid_error("conversation read deadline overflow"))?;
        self.begin_operation_until_with(deadline, |_| {})
    }
    pub fn begin_operation_until_with(
        &mut self,
        deadline: Instant,
        register_cancellation: impl FnOnce(ConversationReadCancellation),
    ) -> CwrResult<ConversationReadOperation<'_>> {
        let cancellation = ConversationReadCancellation::pending();
        register_cancellation(cancellation.clone());
        self.begin_operation_until(deadline, cancellation)
    }
    pub(crate) fn begin_operation_until(
        &mut self,
        deadline: Instant,
        cancellation: ConversationReadCancellation,
    ) -> CwrResult<ConversationReadOperation<'_>> {
        let finite = self
            .factory
            .begin(cancellation.storage_control_for_path(deadline, &self.path))?;
        let operation = ConversationReadOperation {
            include_execution_conversations: false,
            owner_context_filter: None,
            path: &self.path,
            deadline,
            cancellation,
            finite: FiniteSnapshot::Owned(Box::new(finite)),
        };
        operation.ensure_active()?;
        Ok(operation)
    }
}
impl ConversationReadOperation<'_> {
    pub(crate) fn including_execution_conversations(mut self, include: bool) -> Self {
        self.include_execution_conversations = include;
        self
    }
    pub fn cancellation(&self) -> ConversationReadCancellation {
        self.cancellation.clone()
    }
    fn finite_snapshot(&self) -> CwrResult<&cwpdb_store::conversation::ReadSnapshot> {
        self.ensure_active()?;
        Ok(&self.finite)
    }
    fn ensure_active(&self) -> CwrResult<()> {
        self.cancellation.ensure_active(self.deadline)
    }
    pub fn scope_to_owner(mut self, request: &ConversationSummaryPageRequest) -> Self {
        self.owner_context_filter = request.owner_filter();
        self
    }
}
#[cfg(test)]
#[path = "read/participant_tests.rs"]
mod participant_tests;
#[cfg(test)]
#[path = "read_tests.rs"]
mod tests;
