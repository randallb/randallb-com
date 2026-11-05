use cwr::v2026_09::{conversation::RetainedConversation, host::HostEvent};

pub(super) fn root_conversation_snapshot(
    source_ref: &str,
    conversations: Vec<RetainedConversation>,
) -> HostEvent {
    HostEvent::ConversationSnapshot {
        source_ref: source_ref.into(),
        conversations: conversations
            .into_iter()
            .filter(|conversation| conversation.parent_conversation_ref.is_none())
            .collect(),
    }
}

pub(super) fn root_conversation_upsert(
    source_ref: &str,
    conversation: RetainedConversation,
) -> Option<HostEvent> {
    conversation
        .parent_conversation_ref
        .is_none()
        .then(|| HostEvent::ConversationUpsert {
            source_ref: source_ref.into(),
            conversation,
        })
}
