use super::*;
impl NativeReadSnapshot {
    pub(super) fn select_pages(&self, operation: &ReadSelectionOperation) -> CwpdbResult<Value> {
        Ok(match operation {
            ReadSelectionOperation::ContentMessage {
                conversation_ref,
                message_ref,
                include_execution,
            } => serde_json::to_value(self.content_message(
                conversation_ref,
                message_ref,
                *include_execution,
            )?)?,
            ReadSelectionOperation::ContentPositions {
                conversation_ref,
                window,
                include_execution,
            } => serde_json::to_value(self.content_positions(
                conversation_ref,
                window,
                *include_execution,
            )?)?,
            ReadSelectionOperation::CompletionPage {
                conversation_ref,
                window,
                include_execution,
            } => serde_json::to_value(self.completion_page(
                conversation_ref,
                window,
                *include_execution,
            )?)?,
            ReadSelectionOperation::TimelinePage {
                conversation_ref,
                window,
                include_execution,
            } => serde_json::to_value(self.timeline_page(
                conversation_ref,
                window,
                *include_execution,
            )?)?,
            ReadSelectionOperation::LegacyTimelinePage {
                conversation_ref,
                window,
                include_execution,
            } => serde_json::to_value(self.legacy_timeline_page(
                conversation_ref,
                window,
                *include_execution,
            )?)?,
            ReadSelectionOperation::LegacyTimelineMessage {
                conversation_ref,
                key,
                message_ref,
                include_execution,
            } => serde_json::to_value(self.legacy_timeline_message(
                conversation_ref,
                key,
                message_ref,
                *include_execution,
            )?)?,
            ReadSelectionOperation::SummaryWindow { request } => {
                serde_json::to_value(self.summary_window(request)?)?
            }
            ReadSelectionOperation::SummaryRow {
                reference,
                workspace,
            } => serde_json::to_value(
                self.summary_row(
                    reference,
                    workspace
                        .as_ref()
                        .map(|(owner, actor)| (owner.as_deref(), actor.as_str())),
                )?,
            )?,
            ReadSelectionOperation::SummaryMessageRecord {
                conversation_ref,
                message_ref,
            } => serde_json::to_value(self.summary_message_record(conversation_ref, message_ref)?)?,
            ReadSelectionOperation::HistoryRead {
                conversation_ref,
                selection,
                participant_actor_ref,
                include_execution,
            } => serde_json::to_value(self.history_read(
                conversation_ref,
                selection.clone(),
                participant_actor_ref.clone(),
                *include_execution,
            )?)?,
            _ => return Err("invalid finite selection dispatch".into()),
        })
    }
}
