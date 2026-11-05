//! Existing common checks exercise the public finite read facade on each backend.
use super::*;
impl ConversationAdmissionStore {
    pub(crate) fn canonical_content_page(
        &self,
        reference: &str,
        page: &ConversationRecordPageRequest,
    ) -> CwrResult<ConversationContentPage> {
        let mut reader = self.read_store()?;
        reader
            .begin_operation(std::time::Duration::from_secs(2))?
            .conversation_content(reference, page.window())
    }
    pub(crate) fn canonical_completion_page(
        &self,
        reference: &str,
        after: i64,
        through: Option<i64>,
        revision: Option<&str>,
        first: usize,
    ) -> CwrResult<ConversationCompletionPage> {
        let mut reader = self.read_store()?;
        reader
            .begin_operation(std::time::Duration::from_secs(2))?
            .conversation_completions(
                reference,
                self.runtime_root_ref(),
                after,
                through,
                revision,
                first,
            )
    }
}
