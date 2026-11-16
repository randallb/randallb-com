use super::*;

impl ConversationAdmissionStore {
    pub fn is_direct_action(&self, record: &ConversationAdmissionRecord) -> CwrResult<bool> {
        let conversation = self.full_conversation(&record.conversation_ref)?;
        let source = conversation
            .as_ref()
            .and_then(|conversation| {
                conversation
                    .messages
                    .iter()
                    .find(|message| message.message_ref == record.source_message_ref)
            })
            .ok_or_else(|| invalid_error("admission source message is unavailable"))?;
        Ok(matches!(
            source.action.as_ref(),
            Some(ConversationActionEvent::Call { .. })
        ))
    }
}
