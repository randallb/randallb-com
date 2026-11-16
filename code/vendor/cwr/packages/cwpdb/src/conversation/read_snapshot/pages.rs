//! Typed finite page methods over native and revision-pinned remote views.
use super::*;
impl ReadSnapshot {
    pub fn content_message(
        &self,
        conversation: &str,
        message: &str,
        include_execution: bool,
    ) -> CwpdbResult<Option<ContentItemFacts>> {
        match self {
            Self::Native(snapshot) => {
                snapshot.content_message(conversation, message, include_execution)
            }
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ContentMessage {
                conversation_ref: conversation.into(),
                message_ref: message.into(),
                include_execution,
            }),
        }
    }
    pub fn content_positions(
        &self,
        conversation: &str,
        window: &PositionWindow,
        include_execution: bool,
    ) -> CwpdbResult<Option<HistoryReadFacts>> {
        match self {
            Self::Native(snapshot) => {
                snapshot.content_positions(conversation, window, include_execution)
            }
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ContentPositions {
                conversation_ref: conversation.into(),
                window: window.clone(),
                include_execution,
            }),
        }
    }
    pub fn completion_page(
        &self,
        conversation: &str,
        window: &CompletionWindow,
        include_execution: bool,
    ) -> CwpdbResult<Option<CompletionPageFacts>> {
        match self {
            Self::Native(snapshot) => {
                snapshot.completion_page(conversation, window, include_execution)
            }
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::CompletionPage {
                conversation_ref: conversation.into(),
                window: window.clone(),
                include_execution,
            }),
        }
    }
    pub fn timeline_page(
        &self,
        conversation: &str,
        window: &PositionWindow,
        include_execution: bool,
    ) -> CwpdbResult<Option<TimelineReadFacts>> {
        match self {
            Self::Native(snapshot) => {
                snapshot.timeline_page(conversation, window, include_execution)
            }
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::TimelinePage {
                conversation_ref: conversation.into(),
                window: window.clone(),
                include_execution,
            }),
        }
    }
    pub fn legacy_timeline_page(
        &self,
        conversation: &str,
        window: &PositionWindow,
        include_execution: bool,
    ) -> CwpdbResult<TimelineReadFacts> {
        match self {
            Self::Native(snapshot) => {
                snapshot.legacy_timeline_page(conversation, window, include_execution)
            }
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::LegacyTimelinePage {
                conversation_ref: conversation.into(),
                window: window.clone(),
                include_execution,
            }),
        }
    }
    pub fn legacy_timeline_message(
        &self,
        conversation: &str,
        key: &PositionCursor,
        message: &str,
        include_execution: bool,
    ) -> CwpdbResult<Option<Value>> {
        match self {
            Self::Native(snapshot) => {
                snapshot.legacy_timeline_message(conversation, key, message, include_execution)
            }
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::LegacyTimelineMessage {
                    conversation_ref: conversation.into(),
                    key: key.clone(),
                    message_ref: message.into(),
                    include_execution,
                })
            }
        }
    }
}
