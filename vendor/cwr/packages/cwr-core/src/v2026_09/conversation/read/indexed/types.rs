use serde::{Deserialize, Serialize};

use super::super::invalid;
use crate::{v2026_09::conversation::ConversationMessage, CwrResult};

pub const MAX_CONVERSATION_RECORD_PAGE_SIZE: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationPositionKey {
    pub position: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ConversationRecordWindow<'key> {
    pub after: Option<&'key ConversationPositionKey>,
    pub before: Option<&'key ConversationPositionKey>,
    pub first: Option<usize>,
    pub last: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationRecordPageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ConversationPositionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ConversationPositionKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<usize>,
}

impl ConversationRecordPageRequest {
    pub fn window(&self) -> ConversationRecordWindow<'_> {
        ConversationRecordWindow {
            after: self.after.as_ref(),
            before: self.before.as_ref(),
            first: self.first,
            last: self.last,
        }
    }

    pub fn validate(&self) -> CwrResult<()> {
        validate_record_window(self.window())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationRecordReadRequest {
    Summary {
        conversation_ref: String,
        workspace_owner_context_ref: Option<String>,
        participant_actor_ref: String,
    },
    Resource {
        resource_ref: String,
    },
    Completions {
        conversation_ref: String,
        after_sequence: i64,
        through_sequence: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeline_revision: Option<String>,
        first: usize,
    },
    Content {
        conversation_ref: String,
        page: ConversationRecordPageRequest,
    },
    Timeline {
        conversation_ref: String,
        page: ConversationRecordPageRequest,
    },
}

impl ConversationRecordReadRequest {
    pub fn conversation_ref(&self) -> Option<&str> {
        match self {
            Self::Resource { .. } => None,
            Self::Summary {
                conversation_ref, ..
            }
            | Self::Content {
                conversation_ref, ..
            }
            | Self::Timeline {
                conversation_ref, ..
            }
            | Self::Completions {
                conversation_ref, ..
            } => Some(conversation_ref),
        }
    }

    pub fn validate(&self) -> CwrResult<()> {
        if let Self::Completions {
            after_sequence,
            through_sequence,
            timeline_revision,
            first,
            ..
        } = self
        {
            if *after_sequence < 0
                || through_sequence.is_some_and(|end| end < *after_sequence)
                || !(1..=200).contains(first)
            {
                return invalid("invalid completion history window");
            }
            if timeline_revision
                .as_deref()
                .is_some_and(|revision| revision.trim().is_empty())
            {
                return invalid("completion timeline revision must not be empty");
            }
            return Ok(());
        }
        let page = match self {
            Self::Content { page, .. } | Self::Timeline { page, .. } => page,
            Self::Resource { .. } | Self::Summary { .. } => return Ok(()),
            Self::Completions { .. } => unreachable!(),
        };
        page.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationContent {
    pub key: ConversationPositionKey,
    pub message: ConversationMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationContentPage {
    pub content: Vec<ConversationContent>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub total_count: usize,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationTimelineAnchor {
    Message {
        key: ConversationPositionKey,
        message_ref: String,
    },
    Turn {
        key: ConversationPositionKey,
        invocation_ref: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationTimelinePage {
    /// The authoritative visible items, in display order. Dependencies below
    /// provide rendering context and must not add or remove visible items.
    pub anchors: Vec<ConversationTimelineAnchor>,
    /// Selected messages and the reply/source ancestry needed for entity lookup.
    pub messages: Vec<ConversationMessage>,
    /// Selected turns' work, including descendants. Ancestors outside that work
    /// carry relationship/lifecycle context, not their other work branches.
    pub activity: Vec<crate::v2026_09::conversation::ConversationActivity>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub total_count: usize,
    /// Identifies anchor membership/order; ordinary progress and lifecycle
    /// updates remain readable without invalidating a pagination cursor.
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationRecordPage {
    Summary {
        summary: Option<super::super::ConversationSummary>,
    },
    Resource {
        #[schemars(with = "Option<serde_json::Value>")]
        resource: Option<Box<super::super::ConversationResourceSnapshot>>,
    },
    Completions {
        page: ConversationCompletionPage,
    },
    Content {
        page: ConversationContentPage,
    },
    Timeline {
        page: ConversationTimelinePage,
    },
}

pub(super) fn validate_record_window(window: ConversationRecordWindow<'_>) -> CwrResult<()> {
    if window.first.is_none() && window.last.is_none() {
        return invalid("conversation record page requires first or last");
    }
    for count in [window.first, window.last].into_iter().flatten() {
        if count > MAX_CONVERSATION_RECORD_PAGE_SIZE {
            return invalid(format!(
                "conversation record page size must not exceed {MAX_CONVERSATION_RECORD_PAGE_SIZE}"
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationCompletionPage {
    pub runtime_ref: String,
    pub through_sequence: i64,
    pub revision: String,
    pub items: Vec<super::super::CompletedWorkSummary>,
    pub next_after: i64,
    pub has_next_page: bool,
}
