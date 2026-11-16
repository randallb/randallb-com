//! Filters are applied to shared presentation values before selecting a page.
use super::*;
use crate::v2026_09::conversation::{
    ConversationTimelineConnectionOwner, ConversationTimelineContent, ConversationTimelineEdge,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationTimelineSourceType {
    Message,
    ActionInvocation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationTimelineChildRole {
    Commentary,
    FinalReply,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPublicationFilter {
    pub search: Option<String>,
    pub source_types: Vec<ConversationTimelineSourceType>,
    pub child_role: Option<ConversationTimelineChildRole>,
}
impl ConversationPublicationFilter {
    pub fn normalized(mut self) -> CwrResult<Self> {
        self.search = self
            .search
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty());
        if self.search.as_ref().is_some_and(|s| s.len() > 1024) {
            return invalid("timeline search exceeds 1024 bytes");
        }
        self.source_types.sort();
        self.source_types.dedup();
        // Selecting all supported kinds is equivalent to no kind restriction.
        if self.source_types.len() == 2 {
            self.source_types.clear();
        }
        Ok(self)
    }
    fn matches_kind(&self, content: &ConversationTimelineContent) -> bool {
        let kind = match content {
            ConversationTimelineContent::Message { .. } => ConversationTimelineSourceType::Message,
            ConversationTimelineContent::Invocation { .. } => {
                ConversationTimelineSourceType::ActionInvocation
            }
        };
        (self.source_types.is_empty() || self.source_types.contains(&kind))
            && (self.child_role.is_none() || kind == ConversationTimelineSourceType::Message)
    }
    fn matches(&self, value: &ConversationPublication) -> bool {
        self.matches_kind(&value.item.content)
            && self
                .search
                .as_ref()
                .is_none_or(|search| value.text.to_lowercase().contains(search))
    }
}
impl ConversationPublicationSnapshot {
    /// Scan siblings in bounded indexed batches, hydrating only this connection.
    /// The operation deadline and cancellation cover source matching and counts.
    /// No descendant is visited and no unbounded result collection is allocated.
    pub fn filtered_page(
        &self,
        conversation: &str,
        connection: Option<&str>,
        request: &ConversationTimelinePageRequest,
        filter: &ConversationPublicationFilter,
        count: bool,
    ) -> CwrResult<Option<ConversationPublicationPage>> {
        let filter = filter.clone().normalized()?;
        if filter == ConversationPublicationFilter::default() {
            return self.page(conversation, connection, request, count);
        }
        self.read(|op| {
            if !self.visible(op, conversation)? {
                return Ok(None);
            }
            let Some(root) = op.timeline_root_ref(conversation)? else {
                return Ok(None);
            };
            let connection = connection.unwrap_or(&root);
            // Validate original window, including both cursor ownership and ordering.
            let mut result =
                op.conversation_timeline_connection(conversation, connection, request)?;
            let child = filter
                .child_role
                .map(|role| ChildRoleContext::load(op, conversation, &result.owner, role))
                .transpose()?;
            let backwards = request.last.is_some();
            let limit = request.first.or(request.last).expect("validated page") as usize;
            let mut window = request.clone();
            window.first = (!backwards).then_some(100);
            window.last = backwards.then_some(100);
            let mut matches = Vec::new();
            loop {
                let batch =
                    op.conversation_timeline_connection(conversation, connection, &window)?;
                let more = if backwards {
                    batch.page_info.has_previous_page
                } else {
                    batch.page_info.has_next_page
                };
                let boundary = if backwards {
                    batch.page_info.start_cursor.clone()
                } else {
                    batch.page_info.end_cursor.clone()
                };
                let items: Box<dyn Iterator<Item = ConversationTimelineEdge>> = if backwards {
                    Box::new(batch.edges.into_iter().rev())
                } else {
                    Box::new(batch.edges.into_iter())
                };
                for edge in items {
                    op.ensure_active()?;
                    // Placement kinds are retained independently of payloads.
                    if !filter.matches_kind(&edge.item.content)
                        || !child
                            .as_ref()
                            .map(|child| child.matches(op, edge.item.content.reference()))
                            .transpose()?
                            .unwrap_or(true)
                    {
                        continue;
                    }
                    let publication = filter
                        .search
                        .as_ref()
                        .map(|_| self.project(op, conversation, edge.item.clone()))
                        .transpose()?;
                    if publication
                        .as_ref()
                        .is_none_or(|value| filter.matches(value))
                    {
                        matches.push((edge, publication));
                    }
                    if matches.len() > limit {
                        break;
                    }
                }
                if matches.len() > limit || !more {
                    break;
                }
                if backwards {
                    window.before = boundary;
                } else {
                    window.after = boundary;
                }
            }
            let truncated = matches.len() > limit;
            matches.truncate(limit);
            if backwards {
                matches.reverse();
            }
            result.edges = matches.iter().map(|(edge, _)| edge.clone()).collect();
            result.page_info.has_previous_page = backwards && truncated;
            result.page_info.has_next_page = !backwards && truncated;
            result.page_info.start_cursor = result.edges.first().map(|e| e.cursor.clone());
            result.page_info.end_cursor = result.edges.last().map(|e| e.cursor.clone());
            let total_count = count
                .then(|| self.filtered_count(op, conversation, connection, &filter, child.as_ref()))
                .transpose()?;
            let publications = matches
                .into_iter()
                .map(|(edge, publication)| {
                    publication.map_or_else(|| self.project(op, conversation, edge.item), Ok)
                })
                .collect::<CwrResult<Vec<_>>>()?;
            Ok(Some(ConversationPublicationPage {
                revision: format!(
                    "{}:{}",
                    connection,
                    op.snapshot_position(conversation)?.commit_revision
                ),
                page: result,
                publications,
                total_count,
            }))
        })
    }
    fn filtered_count(
        &self,
        op: &ConversationReadOperation<'_>,
        conversation: &str,
        connection: &str,
        filter: &ConversationPublicationFilter,
        child: Option<&ChildRoleContext>,
    ) -> CwrResult<usize> {
        let mut window = ConversationTimelinePageRequest {
            first: Some(100),
            ..Default::default()
        };
        let mut total = 0usize;
        loop {
            let batch = op.conversation_timeline_connection(conversation, connection, &window)?;
            for edge in &batch.edges {
                op.ensure_active()?;
                if !filter.matches_kind(&edge.item.content)
                    || !child
                        .map(|child| child.matches(op, edge.item.content.reference()))
                        .transpose()?
                        .unwrap_or(true)
                    || (filter.search.is_some()
                        && !filter.matches(&self.project(op, conversation, edge.item.clone())?))
                {
                    continue;
                }
                total = total
                    .checked_add(1)
                    .ok_or_else(|| invalid_error("timeline count overflow"))?;
            }
            if !batch.page_info.has_next_page {
                break;
            }
            window.after = batch.page_info.end_cursor;
        }
        Ok(total)
    }
}

struct ChildRoleContext {
    role: ConversationTimelineChildRole,
    invocation_ref: String,
    target_actor_ref: Option<String>,
    source_message_ref: Option<String>,
}

impl ChildRoleContext {
    fn load(
        op: &ConversationReadOperation<'_>,
        conversation: &str,
        owner: &ConversationTimelineConnectionOwner,
        role: ConversationTimelineChildRole,
    ) -> CwrResult<Self> {
        let ConversationTimelineConnectionOwner::TimelineItem { item_ref } = owner else {
            return invalid("child role filter requires an invocation's child connection");
        };
        let Some(item) = op.conversation_timeline_item(conversation, item_ref)? else {
            return invalid("child role filter owner is unavailable");
        };
        let ConversationTimelineContent::Invocation { invocation_ref } = item.content else {
            return invalid("child role filter requires an invocation's child connection");
        };
        let metadata = op
            .finite_snapshot()?
            .invocation_target_source(&invocation_ref)?;
        Ok(Self {
            role,
            invocation_ref,
            target_actor_ref: metadata.as_ref().map(|(target, _)| target.clone()),
            source_message_ref: metadata.and_then(|(_, source)| source),
        })
    }

    fn matches(&self, op: &ConversationReadOperation<'_>, reference: &str) -> CwrResult<bool> {
        let Some(target_actor_ref) = self.target_actor_ref.as_ref() else {
            return Ok(false);
        };
        Ok(match self.role {
            ConversationTimelineChildRole::Commentary => op
                .finite_snapshot()?
                .message_is_commentary(reference, &self.invocation_ref)?,
            ConversationTimelineChildRole::FinalReply => {
                op.finite_snapshot()?.message_is_final_reply(
                    reference,
                    target_actor_ref,
                    self.source_message_ref.as_deref(),
                )?
            }
        })
    }
}
