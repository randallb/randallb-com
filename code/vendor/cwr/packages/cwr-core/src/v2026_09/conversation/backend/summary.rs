//! Existing participation checks exercise the same public finite summary interface.
use super::*;
impl ConversationAdmissionStore {
    pub(crate) fn scoped_conversation_summary(
        &self,
        reference: &str,
        owner: Option<&str>,
        actor: &str,
    ) -> CwrResult<Option<ConversationSummary>> {
        let mut reader = self.read_store()?;
        reader
            .begin_operation(std::time::Duration::from_secs(2))?
            .workspace_conversation_summary(reference, owner, actor)
    }
}
