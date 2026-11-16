//! The public read facade projects finite facts from its original frozen selection.
use super::backend::{HistoryProjection, HistorySource};
use super::*;
use cwpdb_store::conversation::{
    HistoryReadFacts, HistorySelection, NativeReadSnapshot, ReadControl,
};
trait FrozenHistorySelection {
    fn history_read(
        &self,
        reference: &str,
        selection: HistorySelection,
        participant: Option<String>,
        include_execution: bool,
    ) -> CwrResult<Option<HistoryReadFacts>>;
}
impl FrozenHistorySelection for NativeReadSnapshot {
    fn history_read(
        &self,
        reference: &str,
        selection: HistorySelection,
        participant: Option<String>,
        include_execution: bool,
    ) -> CwrResult<Option<HistoryReadFacts>> {
        NativeReadSnapshot::history_read(self, reference, selection, participant, include_execution)
    }
}
impl FrozenHistorySelection for cwpdb_store::conversation::ReadSnapshot {
    fn history_read(
        &self,
        reference: &str,
        selection: HistorySelection,
        participant: Option<String>,
        include_execution: bool,
    ) -> CwrResult<Option<HistoryReadFacts>> {
        Self::history_read(self, reference, selection, participant, include_execution)
    }
}
struct NativeHistoryReader<'a> {
    snapshot: &'a dyn FrozenHistorySelection,
    reference: &'a str,
    include_execution: bool,
    control: ReadControl,
    archive: bool,
}
impl BrowseControl for NativeHistoryReader<'_> {
    fn ensure_active(&self) -> CwrResult<()> {
        self.control.check()
    }
}
impl HistorySource for NativeHistoryReader<'_> {
    fn facts(&self, selection: HistorySelection) -> CwrResult<HistoryReadFacts> {
        self.ensure_active()?;
        let selection = if self.archive {
            match selection {
                HistorySelection::RetainedMessage { message_ref } => {
                    HistorySelection::ArchiveMessage { message_ref }
                }
                other => other,
            }
        } else {
            selection
        };
        self.snapshot
            .history_read(self.reference, selection, None, self.include_execution)?
            .ok_or_else(|| invalid_error(format!("unknown conversation ref {}", self.reference)))
    }
}
impl ConversationReadOperation<'_> {
    fn history_reader<'a>(
        &'a self,
        reference: &'a str,
        archive: bool,
    ) -> CwrResult<NativeHistoryReader<'a>> {
        self.ensure_active()?;
        Ok(NativeHistoryReader {
            snapshot: self.finite_snapshot()?,
            reference,
            include_execution: self.include_execution_conversations,
            control: self.cancellation.storage_control(self.deadline),
            archive,
        })
    }
    pub fn conversation_history(
        &self,
        reference: &str,
        window: ConversationRecordWindow<'_>,
    ) -> CwrResult<ConversationHistoryPage> {
        self.history_reader(reference, true)?
            .history(window, "conversation_history")
    }
    pub fn conversation_browse_page(
        &self,
        reference: &str,
        window: ConversationRecordWindow<'_>,
        bytes: usize,
    ) -> CwrResult<ConversationBrowsePage> {
        self.history_reader(reference, false)?
            .conversation_browse_page(reference, window, bytes)
    }
    pub fn conversation_history_around(
        &self,
        reference: &str,
        message: &str,
        before: usize,
        after: usize,
        bytes: usize,
    ) -> CwrResult<ConversationBrowsePage> {
        self.history_reader(reference, false)?
            .conversation_history_around(reference, message, before, after, bytes)
    }
    pub fn conversation_message_excerpt(
        &self,
        reference: &str,
        message: &str,
        offset: usize,
        payload_offset: usize,
        bytes: usize,
    ) -> CwrResult<ConversationBrowseEntry> {
        self.history_reader(reference, false)?
            .conversation_message_excerpt(reference, message, offset, payload_offset, bytes)
    }
    pub fn conversation_message_links(
        &self,
        reference: &str,
        message: &str,
        after: Option<&str>,
        revision: Option<&str>,
        limit: usize,
    ) -> CwrResult<ConversationMessageLinks> {
        self.history_reader(reference, false)?
            .conversation_message_links(reference, message, after, revision, limit)
    }
    pub fn conversation_history_search(
        &self,
        reference: &str,
        query: &str,
        after: Option<i64>,
        revision: Option<&str>,
        limit: usize,
        bytes: usize,
    ) -> CwrResult<ConversationSearchPage> {
        self.history_reader(reference, false)?
            .conversation_history_search(reference, query, after, revision, limit, bytes)
    }
}
