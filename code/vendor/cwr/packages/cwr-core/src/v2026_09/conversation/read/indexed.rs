use super::{invalid, invalid_error, ConversationReadOperation};
use crate::{v2026_09::conversation::ConversationMessage, CwrResult};

mod history;
pub(crate) use history::backend::HistoryProjection;
pub use history::{
    ConversationBrowseEntry, ConversationBrowsePage, ConversationHistoryEntry,
    ConversationHistoryPage, ConversationMessageLinks, ConversationSearchPage,
    MAX_CONVERSATION_ACTION_EXCERPT_BYTES, MAX_CONVERSATION_BROWSE_PAGE_SIZE,
    MAX_CONVERSATION_BROWSE_RESPONSE_BYTES, MAX_CONVERSATION_EXCERPT_BYTES,
    MAX_CONVERSATION_SEARCH_QUERY_CHARACTERS,
};

mod canonical_timeline;
mod legacy_timeline;
pub use legacy_timeline::ConversationLegacyTimelinePage;

mod types;
pub use types::{
    ConversationCompletionPage, ConversationContent, ConversationContentPage,
    ConversationPositionKey, ConversationRecordPage, ConversationRecordPageRequest,
    ConversationRecordReadRequest, ConversationRecordWindow, ConversationTimelineAnchor,
    ConversationTimelinePage, MAX_CONVERSATION_RECORD_PAGE_SIZE,
};

mod finite;

#[cfg(test)]
#[path = "indexed_tests.rs"]
mod tests;

mod targeting;
