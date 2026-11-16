use super::*;

impl ConversationReadOperation<'_> {
    /// Select the root without counting or flattening its placements.
    pub fn conversation_timeline_root_page(
        &self,
        conversation_ref: &str,
        page: &ConversationTimelinePageRequest,
    ) -> CwrResult<ConversationTimelineConnectionPage> {
        self.require_conversation_revision(conversation_ref)?;
        let root = self
            .timeline_root_ref(conversation_ref)?
            .ok_or_else(|| invalid_error("conversation has no retained timeline"))?;
        self.conversation_timeline_connection(conversation_ref, &root, page)
    }

    /// Placement ownership is checked before reading one visible message.
    /// Full payloads are loaded only when requested, independently of paging.
    pub fn conversation_timeline_placed_message(
        &self,
        conversation_ref: &str,
        item_ref: &str,
    ) -> CwrResult<Option<ConversationMessage>> {
        let Some(item) = self.conversation_timeline_item(conversation_ref, item_ref)? else {
            return Ok(None);
        };
        let ConversationTimelineContent::Message { message_ref } = item.content else {
            return Ok(None);
        };
        self.ensure_active()?;
        let resource = self.finite_snapshot()?.resource(&message_ref)?;
        Ok(match resource {
            Some(facts) if facts.conversation_ref == conversation_ref => match facts.resource {
                cwpdb_store::conversation::ResourceRecordFacts::Message { record, .. } => {
                    Some(serde_json::from_value(record)?)
                }
                _ => None,
            },
            _ => None,
        })
    }

    /// Count siblings only when the consumer explicitly requests an exact count.
    pub fn conversation_timeline_connection_count(
        &self,
        conversation_ref: &str,
        connection_ref: &str,
    ) -> CwrResult<usize> {
        self.require_conversation_revision(conversation_ref)?;
        let root = self
            .timeline_root_ref(conversation_ref)?
            .ok_or_else(|| invalid_error("conversation has no retained timeline"))?;
        self.timeline_connection_metadata(conversation_ref, &root, connection_ref)?;
        self.finite_snapshot()?
            .timeline_connection_count(conversation_ref, connection_ref)
    }

    /// Resolve one reachable placement without loading the retained topology.
    pub fn conversation_timeline_item(
        &self,
        conversation_ref: &str,
        item_ref: &str,
    ) -> CwrResult<Option<ConversationTimelineItem>> {
        self.require_conversation_revision(conversation_ref)?;
        self.finite_snapshot()?
            .timeline_item(conversation_ref, item_ref)?
            .map(|value| Ok(serde_json::from_value(value)?))
            .transpose()
    }
}
