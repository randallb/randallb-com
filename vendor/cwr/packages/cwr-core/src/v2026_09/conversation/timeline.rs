use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

use cwr_core::CwrRef;
use serde::{Deserialize, Serialize};

use crate::CwrResult;

use super::{ConversationAdmissionStore, ConversationRuntimeState, RetainedConversation};

mod migration;
mod rollout;
mod store;
mod topology;
mod validation;

pub use migration::{
    ConversationTimelineMigrationOutcome, ConversationTimelineMigrationSkipReason,
};
pub use rollout::ConversationTimelineRollout;
pub(super) use rollout::{managed_publication_ref, stage_timeline_rollout_changes};
pub use store::ConversationTimelineStore;
#[cfg(test)]
pub(super) use validation::validate_timeline_transition;
pub(super) use validation::{validate_changed_timelines, validate_retained_timelines};

#[cfg(test)]
mod identity_tests;
#[cfg(test)]
mod tests;

pub const CONVERSATION_TIMELINE_CURSOR_VERSION: u32 = 1;
pub const MAX_CONVERSATION_TIMELINE_PAGE_SIZE: u32 = 200;

/// The retained publication topology for one shared conversation.
///
/// Absence on `RetainedConversation` identifies a legacy record. An empty root
/// connection in a present timeline is authoritative.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationTimeline {
    pub root_connection_ref: String,
    pub connections: Vec<ConversationTimelineConnection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationTimelineConnection {
    pub connection_ref: String,
    pub owner: ConversationTimelineConnectionOwner,
    pub revision: u64,
    pub next_sequence: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ConversationTimelineItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationTimelineConnectionOwner {
    Conversation { conversation_ref: String },
    TimelineItem { item_ref: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationTimelineItem {
    pub item_ref: String,
    pub publication_ref: String,
    pub connection_ref: String,
    pub sibling_sequence: u64,
    pub content: ConversationTimelineContent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub child_connection_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationTimelineContent {
    Message { message_ref: String },
    Invocation { invocation_ref: String },
}

impl ConversationTimelineContent {
    pub fn reference(&self) -> &str {
        match self {
            Self::Message { message_ref } => message_ref,
            Self::Invocation { invocation_ref } => invocation_ref,
        }
    }

    fn can_have_children(&self) -> bool {
        matches!(self, Self::Invocation { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationTimelineDisposition {
    Root,
    Child { parent_item_ref: String },
    Undisclosed,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationTimelineCursor {
    pub version: u32,
    pub connection_ref: String,
    pub sibling_sequence: u64,
    pub item_ref: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationTimelinePageRequest {
    pub after: Option<ConversationTimelineCursor>,
    pub before: Option<ConversationTimelineCursor>,
    pub first: Option<u32>,
    pub last: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationTimelineEdge {
    pub cursor: ConversationTimelineCursor,
    pub item: ConversationTimelineItem,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationTimelinePageInfo {
    pub has_previous_page: bool,
    pub has_next_page: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ConversationTimelineCursor")]
    pub start_cursor: Option<ConversationTimelineCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ConversationTimelineCursor")]
    pub end_cursor: Option<ConversationTimelineCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationTimelineConnectionPage {
    pub connection_ref: String,
    pub owner: ConversationTimelineConnectionOwner,
    pub revision: u64,
    pub edges: Vec<ConversationTimelineEdge>,
    pub page_info: ConversationTimelinePageInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationTimelinePublishOutcome {
    Placed(ConversationTimelineItem),
    Existing(ConversationTimelineItem),
    Undisclosed,
}

impl ConversationTimeline {
    pub fn new(
        conversation_ref: impl Into<String>,
        root_connection_ref: impl Into<String>,
    ) -> CwrResult<Self> {
        let conversation_ref = conversation_ref.into();
        let root_connection_ref = root_connection_ref.into();
        let timeline = Self {
            root_connection_ref: root_connection_ref.clone(),
            connections: vec![ConversationTimelineConnection {
                connection_ref: root_connection_ref,
                owner: ConversationTimelineConnectionOwner::Conversation {
                    conversation_ref: conversation_ref.clone(),
                },
                revision: 0,
                next_sequence: 1,
                items: Vec::new(),
            }],
        };
        timeline.validate_for_conversation(&conversation_ref)?;
        Ok(timeline)
    }

    pub fn connection(&self, connection_ref: &str) -> Option<&ConversationTimelineConnection> {
        self.connections
            .iter()
            .find(|connection| connection.connection_ref == connection_ref)
    }

    pub fn item(&self, item_ref: &str) -> Option<&ConversationTimelineItem> {
        self.connections
            .iter()
            .flat_map(|connection| &connection.items)
            .find(|item| item.item_ref == item_ref)
    }

    /// Records one publication decision. Invocation content receives a stable
    /// empty child connection at placement time so later child appends do not
    /// mutate its parent connection.
    ///
    /// `Undisclosed` deliberately creates no placement; callers retain that
    /// decision on the invocation record.
    pub fn publish(
        &mut self,
        publication_ref: impl Into<String>,
        content: ConversationTimelineContent,
        disposition: ConversationTimelineDisposition,
    ) -> CwrResult<ConversationTimelinePublishOutcome> {
        let publication_ref = publication_ref.into();
        validate_absolute_ref("timeline publication", &publication_ref)?;
        validate_absolute_ref("timeline content", content.reference())?;
        if publication_ref == content.reference() {
            return invalid("timeline publication and content refs must be distinct");
        }
        if self.conversation_ref() == Some(publication_ref.as_str())
            || self.connection(&publication_ref).is_some()
            || self.item(&publication_ref).is_some()
            || self.find_content(&publication_ref).is_some()
        {
            return invalid(format!(
                "timeline publication ref {publication_ref} is already in use"
            ));
        }
        if self.structural_identity_in_use(content.reference()) {
            return invalid(format!(
                "timeline content ref {} is already in use",
                content.reference()
            ));
        }

        let target_connection_ref = match &disposition {
            ConversationTimelineDisposition::Root => self.root_connection_ref.clone(),
            ConversationTimelineDisposition::Child { parent_item_ref } => {
                validate_absolute_ref("timeline parent item", parent_item_ref)?;
                self.item(parent_item_ref)
                    .and_then(|item| item.child_connection_ref.clone())
                    .ok_or_else(|| {
                        invalid_error(format!(
                            "timeline parent item {parent_item_ref} has no child connection"
                        ))
                    })?
            }
            ConversationTimelineDisposition::Undisclosed => {
                if self.find_publication(&publication_ref).is_some() {
                    return invalid(format!(
                        "timeline publication {publication_ref} is already disclosed"
                    ));
                }
                return Ok(ConversationTimelinePublishOutcome::Undisclosed);
            }
        };

        if let Some(existing) = self.find_publication(&publication_ref) {
            if existing.content == content && existing.connection_ref == target_connection_ref {
                return Ok(ConversationTimelinePublishOutcome::Existing(
                    existing.clone(),
                ));
            }
            return invalid(format!(
                "timeline publication {publication_ref} was replayed with different content or placement"
            ));
        }

        let (item_ref, child_connection_ref) = Self::placement_refs(&publication_ref, &content);
        validate_absolute_ref("timeline item", &item_ref)?;
        if item_ref == content.reference() || self.identity_in_use(&item_ref) {
            return invalid(format!("timeline item ref {item_ref} is already in use"));
        }
        if let Some(child_connection_ref) = &child_connection_ref {
            validate_absolute_ref("timeline child connection", child_connection_ref)?;
            if child_connection_ref == content.reference()
                || self.identity_in_use(child_connection_ref)
            {
                return invalid(format!(
                    "timeline child connection ref {child_connection_ref} is already in use"
                ));
            }
        }

        let connection_index = self
            .connections
            .iter()
            .position(|connection| connection.connection_ref == target_connection_ref)
            .ok_or_else(|| {
                invalid_error(format!(
                    "unknown timeline connection {target_connection_ref}"
                ))
            })?;
        let sequence = self.connections[connection_index].next_sequence;
        let next_sequence = sequence
            .checked_add(1)
            .ok_or_else(|| invalid_error("timeline sibling sequence overflow"))?;
        let revision = self.connections[connection_index]
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid_error("timeline connection revision overflow"))?;
        let item = ConversationTimelineItem {
            item_ref: item_ref.clone(),
            publication_ref,
            connection_ref: target_connection_ref,
            sibling_sequence: sequence,
            content,
            child_connection_ref: child_connection_ref.clone(),
        };
        let connection = &mut self.connections[connection_index];
        connection.items.push(item.clone());
        connection.next_sequence = next_sequence;
        connection.revision = revision;
        if let Some(connection_ref) = child_connection_ref {
            self.connections.push(ConversationTimelineConnection {
                connection_ref,
                owner: ConversationTimelineConnectionOwner::TimelineItem { item_ref },
                revision: 0,
                next_sequence: 1,
                items: Vec::new(),
            });
        }
        Ok(ConversationTimelinePublishOutcome::Placed(item))
    }

    pub fn page(
        &self,
        connection_ref: &str,
        request: &ConversationTimelinePageRequest,
    ) -> CwrResult<ConversationTimelineConnectionPage> {
        let connection = self.connection(connection_ref).ok_or_else(|| {
            invalid_error(format!("unknown timeline connection {connection_ref}"))
        })?;
        let requested = match (request.first, request.last) {
            (Some(first), None) => (first, false),
            (None, Some(last)) => (last, true),
            _ => return invalid("timeline page must select exactly one of first or last"),
        };
        if requested.0 == 0 || requested.0 > MAX_CONVERSATION_TIMELINE_PAGE_SIZE {
            return invalid(format!(
                "timeline page size must be between 1 and {MAX_CONVERSATION_TIMELINE_PAGE_SIZE}"
            ));
        }

        let mut start = 0;
        let mut end = connection.items.len();
        if let Some(after) = &request.after {
            start = self.resolve_cursor(connection, after)? + 1;
        }
        if let Some(before) = &request.before {
            end = self.resolve_cursor(connection, before)?;
        }
        if start > end {
            return invalid("timeline page cursors select an inverted range");
        }
        let limit = requested.0 as usize;
        let (page_start, page_end) = if requested.1 {
            (end.saturating_sub(limit).max(start), end)
        } else {
            (start, start.saturating_add(limit).min(end))
        };
        let edges = connection.items[page_start..page_end]
            .iter()
            .cloned()
            .map(|item| ConversationTimelineEdge {
                cursor: cursor_for(&item),
                item,
            })
            .collect::<Vec<_>>();
        Ok(ConversationTimelineConnectionPage {
            connection_ref: connection.connection_ref.clone(),
            owner: connection.owner.clone(),
            revision: connection.revision,
            page_info: ConversationTimelinePageInfo {
                has_previous_page: page_start > start,
                has_next_page: page_end < end,
                start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                end_cursor: edges.last().map(|edge| edge.cursor.clone()),
            },
            edges,
        })
    }

    fn find_publication(&self, publication_ref: &str) -> Option<&ConversationTimelineItem> {
        self.connections
            .iter()
            .flat_map(|connection| &connection.items)
            .find(|item| item.publication_ref == publication_ref)
    }

    fn find_content(&self, content_ref: &str) -> Option<&ConversationTimelineItem> {
        self.connections
            .iter()
            .flat_map(|connection| &connection.items)
            .find(|item| item.content.reference() == content_ref)
    }

    fn placement_refs(
        publication_ref: &str,
        content: &ConversationTimelineContent,
    ) -> (String, Option<String>) {
        let item_ref = format!("{}/item", publication_ref.trim_end_matches('/'));
        let child_connection_ref = content
            .can_have_children()
            .then(|| format!("{item_ref}/children"));
        (item_ref, child_connection_ref)
    }

    fn conversation_ref(&self) -> Option<&str> {
        let connection = self.connection(&self.root_connection_ref)?;
        match &connection.owner {
            ConversationTimelineConnectionOwner::Conversation { conversation_ref } => {
                Some(conversation_ref)
            }
            ConversationTimelineConnectionOwner::TimelineItem { .. } => None,
        }
    }

    fn structural_identity_in_use(&self, reference: &str) -> bool {
        self.conversation_ref() == Some(reference)
            || self.connection(reference).is_some()
            || self.item(reference).is_some()
            || self.find_publication(reference).is_some()
    }

    fn identity_in_use(&self, reference: &str) -> bool {
        self.structural_identity_in_use(reference) || self.find_content(reference).is_some()
    }

    fn disposition_for(&self, item: &ConversationTimelineItem) -> ConversationTimelineDisposition {
        if item.connection_ref == self.root_connection_ref {
            return ConversationTimelineDisposition::Root;
        }
        match self
            .connection(&item.connection_ref)
            .map(|connection| &connection.owner)
        {
            Some(ConversationTimelineConnectionOwner::TimelineItem { item_ref }) => {
                ConversationTimelineDisposition::Child {
                    parent_item_ref: item_ref.clone(),
                }
            }
            _ => unreachable!("validated timeline item belongs to a retained connection"),
        }
    }

    fn resolve_cursor(
        &self,
        connection: &ConversationTimelineConnection,
        cursor: &ConversationTimelineCursor,
    ) -> CwrResult<usize> {
        if cursor.version != CONVERSATION_TIMELINE_CURSOR_VERSION {
            return invalid("unsupported conversation timeline cursor version");
        }
        if cursor.connection_ref != connection.connection_ref {
            return invalid("conversation timeline cursor belongs to another connection");
        }
        connection
            .items
            .iter()
            .position(|item| {
                item.item_ref == cursor.item_ref && item.sibling_sequence == cursor.sibling_sequence
            })
            .ok_or_else(|| invalid_error("conversation timeline cursor item is unavailable"))
    }
}

fn cursor_for(item: &ConversationTimelineItem) -> ConversationTimelineCursor {
    ConversationTimelineCursor {
        version: CONVERSATION_TIMELINE_CURSOR_VERSION,
        connection_ref: item.connection_ref.clone(),
        sibling_sequence: item.sibling_sequence,
        item_ref: item.item_ref.clone(),
    }
}

fn validate_absolute_ref(label: &str, reference: &str) -> CwrResult<()> {
    let parsed = CwrRef::parse(reference)
        .map_err(|error| invalid_error(format!("invalid {label} ref {reference}: {error}")))?;
    if reference.starts_with("~@") || parsed.fragment().is_some() {
        return invalid(format!("{label} ref must be absolute and unfragmented"));
    }
    Ok(())
}

fn retained_invocation_source_message_ref<'a>(
    state: &'a ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<&'a str> {
    if let Some(record) = state
        .invocation_ledger
        .invocation(invocation_ref)
        .filter(|record| record.forked_from_invocation_ref.is_some())
    {
        return record
            .source_message_ref
            .as_deref()
            .ok_or_else(|| invalid_error("fork history has no causal source"));
    }
    let execution = super::invocation::execution_conversation_in_state(state, invocation_ref)?;
    let mut sources = execution
        .activity
        .iter()
        .filter_map(|activity| match &activity.event {
            super::ConversationActivityEvent::TurnStarted {
                invocation_ref: started_ref,
                source_message_ref,
                ..
            } if started_ref == invocation_ref => Some(source_message_ref.as_str()),
            _ => None,
        });
    let source = sources
        .next()
        .ok_or_else(|| invalid_error("invocation has no retained causal source"))?;
    if sources.any(|candidate| candidate != source) {
        return invalid("invocation has conflicting retained causal sources");
    }
    Ok(source)
}

fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(invalid_error(message))
}

fn invalid_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidInput, message.into()).into()
}
