//! CWR protocol projection of storage-owned finite pages.
use super::*;
use cwpdb_store::conversation::{
    CompletionWindow, PositionCursor, PositionWindow, TimelineReadFacts,
};
fn window(window: ConversationRecordWindow<'_>) -> PositionWindow {
    let cursor = |key: &ConversationPositionKey| PositionCursor {
        position: key.position,
        revision: key.revision.clone(),
    };
    PositionWindow {
        after: window.after.map(cursor),
        before: window.before.map(cursor),
        first: window.first,
        last: window.last,
    }
}
impl ConversationReadOperation<'_> {
    pub(in crate::v2026_09::conversation::read) fn require_conversation_revision(
        &self,
        reference: &str,
    ) -> CwrResult<()> {
        self.ensure_active()?;
        if !self.finite_snapshot()?.conversation_exists(
            reference,
            self.include_execution_conversations,
            None,
        )? {
            return invalid(format!("unknown conversation ref {reference}"));
        }
        Ok(())
    }
    pub fn conversation_content(
        &self,
        reference: &str,
        requested: ConversationRecordWindow<'_>,
    ) -> CwrResult<ConversationContentPage> {
        self.require_conversation_revision(reference)?;
        let snapshot = self.finite_snapshot()?;
        let facts = snapshot
            .content_positions(
                reference,
                &window(requested),
                self.include_execution_conversations,
            )?
            .ok_or_else(|| invalid_error(format!("unknown conversation ref {reference}")))?;
        let mut content = Vec::new();
        for key in facts.items {
            self.ensure_active()?;
            let item = snapshot
                .content_message(
                    reference,
                    &key.message_ref,
                    self.include_execution_conversations,
                )?
                .ok_or("selected content message disappeared from snapshot")?;
            if item.position != key.position {
                return invalid("content position mismatch");
            }
            let occurred_at_unix_ms = item.occurred_at_unix_ms;
            let record = item.record;
            let message: ConversationMessage = serde_json::from_value(record)?;
            if message.message_ref != key.message_ref {
                return invalid("content message identity mismatch");
            }
            content.push(ConversationContent {
                key: ConversationPositionKey {
                    position: key.position,
                    revision: facts.canonical.then(|| facts.revision.clone()),
                },
                message,
                occurred_at_unix_ms,
            });
        }
        self.ensure_active()?;
        Ok(ConversationContentPage {
            content,
            has_next_page: facts.has_next_page,
            has_previous_page: facts.has_previous_page,
            total_count: usize::try_from(facts.total_count)?,
            revision: facts.revision,
        })
    }
    pub fn conversation_completions(
        &self,
        reference: &str,
        runtime: &str,
        after: i64,
        through: Option<i64>,
        revision: Option<&str>,
        first: usize,
    ) -> CwrResult<ConversationCompletionPage> {
        self.require_conversation_revision(reference)?;
        let facts = self
            .finite_snapshot()?
            .completion_page(
                reference,
                &CompletionWindow {
                    after,
                    through,
                    revision: revision.map(str::to_owned),
                    first,
                },
                self.include_execution_conversations,
            )?
            .ok_or("unknown conversation")?;
        self.ensure_active()?;
        Ok(ConversationCompletionPage {
            runtime_ref: runtime.into(),
            through_sequence: facts.through_sequence,
            revision: facts.revision,
            items: facts
                .items
                .into_iter()
                .map(|i| super::super::CompletedWorkSummary {
                    invocation_ref: i.invocation_ref,
                    sequence: i.sequence,
                })
                .collect(),
            next_after: facts.next_after,
            has_next_page: facts.has_next_page,
        })
    }
    pub fn conversation_timeline(
        &self,
        reference: &str,
        requested: ConversationRecordWindow<'_>,
    ) -> CwrResult<ConversationTimelinePage> {
        self.require_conversation_revision(reference)?;
        let facts = self
            .finite_snapshot()?
            .timeline_page(
                reference,
                &window(requested),
                self.include_execution_conversations,
            )?
            .ok_or("unknown conversation")?;
        let result = project_timeline(facts)?;
        self.ensure_active()?;
        Ok(result)
    }
}
pub(super) fn project_timeline(facts: TimelineReadFacts) -> CwrResult<ConversationTimelinePage> {
    let revision = facts.revision;
    let anchors = facts
        .anchors
        .into_iter()
        .map(|anchor| match anchor {
            cwpdb_store::conversation::TimelineAnchorFacts::Message {
                position,
                message_ref,
            } => ConversationTimelineAnchor::Message {
                key: ConversationPositionKey {
                    position,
                    revision: Some(revision.clone()),
                },
                message_ref,
            },
            cwpdb_store::conversation::TimelineAnchorFacts::Turn {
                position,
                invocation_ref,
            } => ConversationTimelineAnchor::Turn {
                key: ConversationPositionKey {
                    position,
                    revision: Some(revision.clone()),
                },
                invocation_ref,
            },
        })
        .collect();
    Ok(ConversationTimelinePage {
        anchors,
        messages: facts
            .messages
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()?,
        activity: facts
            .activity
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<_, _>>()?,
        has_next_page: facts.has_next_page,
        has_previous_page: facts.has_previous_page,
        total_count: usize::try_from(facts.total_count)?,
        revision,
    })
}
