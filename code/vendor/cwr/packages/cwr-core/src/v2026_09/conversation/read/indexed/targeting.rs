use super::*;
impl ConversationReadOperation<'_> {
    pub(in crate::v2026_09::conversation::read) fn canonical_last_addressed_actor_refs(
        &self,
        conversation: &str,
        root: &str,
        author: &str,
    ) -> CwrResult<Vec<String>> {
        self.finite_snapshot()?
            .last_addressed_actor_refs(conversation, root, author)
    }
}
