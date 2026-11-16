//! Read the retained rootless layout without migrating or expanding dependencies.
use super::*;
use crate::v2026_09::conversation::{
    ConversationLegacyTimelinePage, ConversationMessage, ConversationPositionKey,
    ConversationRecordPageRequest,
};

impl ConversationPublicationSnapshot {
    pub fn timeline_layout(&self, conversation: &str) -> CwrResult<Option<String>> {
        self.read(|op| {
            if !self.visible(op, conversation)? {
                return invalid("conversation is unavailable to the requesting participant");
            }
            op.conversation_timeline_layout(conversation)
        })
    }

    pub fn legacy_page(
        &self,
        conversation: &str,
        request: &ConversationRecordPageRequest,
    ) -> CwrResult<Option<ConversationLegacyTimelinePage>> {
        self.read(|op| {
            if !self.visible(op, conversation)? || op.timeline_root_ref(conversation)?.is_some() {
                return Ok(None);
            }
            op.conversation_legacy_timeline_page(conversation, request)
                .map(Some)
        })
    }

    pub fn legacy_selected_message(
        &self,
        conversation: &str,
        key: &ConversationPositionKey,
        message: &str,
    ) -> CwrResult<Option<ConversationMessage>> {
        self.read(|op| {
            if !self.visible(op, conversation)? {
                return Ok(None);
            }
            op.conversation_legacy_timeline_message(conversation, key, message)
        })
    }
}
