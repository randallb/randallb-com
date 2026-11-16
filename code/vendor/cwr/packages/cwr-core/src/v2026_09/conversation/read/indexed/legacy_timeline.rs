use super::*;
use serde::{Deserialize, Serialize};

/// A page of the retained legacy projection, without canonical placement IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConversationLegacyTimelinePage {
    pub anchors: Vec<ConversationTimelineAnchor>,
    pub has_previous_page: bool,
    pub has_next_page: bool,
    pub revision: String,
}

impl ConversationReadOperation<'_> {
    pub fn conversation_timeline_layout(&self, reference: &str) -> CwrResult<Option<String>> {
        self.require_conversation_revision(reference)?;
        self.timeline_root_ref(reference)
    }
    pub fn conversation_legacy_timeline_page(
        &self,
        reference: &str,
        request: &ConversationRecordPageRequest,
    ) -> CwrResult<ConversationLegacyTimelinePage> {
        request.validate()?;
        let cursor = |k: &ConversationPositionKey| cwpdb_store::conversation::PositionCursor {
            position: k.position,
            revision: k.revision.clone(),
        };
        let facts = self.finite_snapshot()?.legacy_timeline_page(
            reference,
            &cwpdb_store::conversation::PositionWindow {
                after: request.after.as_ref().map(cursor),
                before: request.before.as_ref().map(cursor),
                first: request.first,
                last: request.last,
            },
            self.include_execution_conversations,
        )?;
        let page = super::finite::project_timeline(facts)?;
        self.ensure_active()?;
        Ok(ConversationLegacyTimelinePage {
            anchors: page.anchors,
            has_previous_page: page.has_previous_page,
            has_next_page: page.has_next_page,
            revision: page.revision,
        })
    }
    pub fn conversation_legacy_timeline_message(
        &self,
        reference: &str,
        key: &ConversationPositionKey,
        message: &str,
    ) -> CwrResult<Option<ConversationMessage>> {
        let record = self.finite_snapshot()?.legacy_timeline_message(
            reference,
            &cwpdb_store::conversation::PositionCursor {
                position: key.position,
                revision: key.revision.clone(),
            },
            message,
            self.include_execution_conversations,
        )?;
        self.ensure_active()?;
        record.map(|v| Ok(serde_json::from_value(v)?)).transpose()
    }
}

#[cfg(test)]
mod tests;
