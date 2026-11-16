use super::{invalid, invalid_error, ConversationReadOperation};
use crate::{
    v2026_09::conversation::{
        ConversationMessage, ConversationTimelineConnectionOwner,
        ConversationTimelineConnectionPage, ConversationTimelineContent,
        ConversationTimelineCursor, ConversationTimelineEdge, ConversationTimelineItem,
        ConversationTimelinePageInfo, ConversationTimelinePageRequest,
        CONVERSATION_TIMELINE_CURSOR_VERSION, MAX_CONVERSATION_TIMELINE_PAGE_SIZE,
    },
    CwrResult,
};

impl ConversationReadOperation<'_> {
    /// Reads one retained connection directly from CWPDB without hydrating any
    /// other conversation or connection into memory.
    pub fn conversation_timeline_connection(
        &self,
        conversation_ref: &str,
        connection_ref: &str,
        request: &ConversationTimelinePageRequest,
    ) -> CwrResult<ConversationTimelineConnectionPage> {
        self.require_conversation_revision(conversation_ref)?;
        let root_connection_ref = self
            .timeline_root_ref(conversation_ref)?
            .ok_or_else(|| invalid_error("conversation has no retained timeline"))?;
        let (owner, revision) = self.timeline_connection_metadata(
            conversation_ref,
            &root_connection_ref,
            connection_ref,
        )?;
        let (limit, backwards) = validate_page_request(request)?;
        let after = request
            .after
            .as_ref()
            .map(|cursor| self.timeline_cursor_position(connection_ref, cursor))
            .transpose()?;
        let before = request
            .before
            .as_ref()
            .map(|cursor| self.timeline_cursor_position(connection_ref, cursor))
            .transpose()?;
        if after
            .zip(before)
            .is_some_and(|(after, before)| after >= before)
        {
            return invalid("timeline page cursors select an inverted range");
        }
        let mut items = self.timeline_connection_items(
            connection_ref,
            after.unwrap_or(i64::MIN),
            before.unwrap_or(i64::MAX),
            limit,
            backwards,
        )?;
        let truncated = items.len() > limit;
        items.truncate(limit);
        if backwards {
            items.reverse();
        }
        let edges = items
            .into_iter()
            .map(|item| ConversationTimelineEdge {
                cursor: cursor_for(&item),
                item,
            })
            .collect::<Vec<_>>();
        self.ensure_active()?;
        Ok(ConversationTimelineConnectionPage {
            connection_ref: connection_ref.to_owned(),
            owner,
            revision,
            page_info: ConversationTimelinePageInfo {
                has_previous_page: backwards && truncated,
                has_next_page: !backwards && truncated,
                start_cursor: edges.first().map(|edge| edge.cursor.clone()),
                end_cursor: edges.last().map(|edge| edge.cursor.clone()),
            },
            edges,
        })
    }

    pub(in crate::v2026_09::conversation::read) fn timeline_root_ref(
        &self,
        conversation_ref: &str,
    ) -> CwrResult<Option<String>> {
        self.finite_snapshot()?
            .timeline_root(conversation_ref, self.include_execution_conversations)
    }

    fn timeline_connection_metadata(
        &self,
        conversation_ref: &str,
        root_connection_ref: &str,
        connection_ref: &str,
    ) -> CwrResult<(ConversationTimelineConnectionOwner, u64)> {
        let (owner, revision) = self.finite_snapshot()?.timeline_connection_metadata(
            conversation_ref,
            root_connection_ref,
            connection_ref,
        )?;
        Ok((serde_json::from_value(owner)?, revision))
    }

    fn timeline_cursor_position(
        &self,
        connection_ref: &str,
        cursor: &ConversationTimelineCursor,
    ) -> CwrResult<i64> {
        if cursor.version != CONVERSATION_TIMELINE_CURSOR_VERSION {
            return invalid("unsupported conversation timeline cursor version");
        }
        if cursor.connection_ref != connection_ref {
            return invalid("conversation timeline cursor belongs to another connection");
        }
        let sequence = i64::try_from(cursor.sibling_sequence)
            .map_err(|_| invalid_error("conversation timeline cursor sequence is too large"))?;
        let exists = self.finite_snapshot()?.timeline_cursor_exists(
            connection_ref,
            sequence,
            &cursor.item_ref,
        )?;
        if !exists {
            return invalid("conversation timeline cursor item is unavailable");
        }
        Ok(sequence)
    }

    fn timeline_connection_items(
        &self,
        connection_ref: &str,
        lower: i64,
        upper: i64,
        limit: usize,
        backwards: bool,
    ) -> CwrResult<Vec<ConversationTimelineItem>> {
        self.finite_snapshot()?
            .timeline_connection_items(connection_ref, lower, upper, limit, backwards)?
            .into_iter()
            .map(|value| Ok(serde_json::from_value(value)?))
            .collect()
    }
}

fn validate_page_request(request: &ConversationTimelinePageRequest) -> CwrResult<(usize, bool)> {
    let (count, backwards) = match (request.first, request.last) {
        (Some(first), None) => (first, false),
        (None, Some(last)) => (last, true),
        _ => return invalid("timeline page must select exactly one of first or last"),
    };
    if count == 0 || count > MAX_CONVERSATION_TIMELINE_PAGE_SIZE {
        return invalid(format!(
            "timeline page size must be between 1 and {MAX_CONVERSATION_TIMELINE_PAGE_SIZE}"
        ));
    }
    Ok((count as usize, backwards))
}

fn cursor_for(item: &ConversationTimelineItem) -> ConversationTimelineCursor {
    ConversationTimelineCursor {
        version: CONVERSATION_TIMELINE_CURSOR_VERSION,
        connection_ref: item.connection_ref.clone(),
        sibling_sequence: item.sibling_sequence,
        item_ref: item.item_ref.clone(),
    }
}

mod lookup;
