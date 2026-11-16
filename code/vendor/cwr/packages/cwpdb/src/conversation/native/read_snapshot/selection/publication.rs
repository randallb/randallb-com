use super::*;
impl NativeReadSnapshot {
    pub(super) fn select_publication(
        &self,
        operation: &ReadSelectionOperation,
    ) -> CwpdbResult<Value> {
        Ok(match operation {
            ReadSelectionOperation::TimelineItemConversation { item_ref } => {
                serde_json::to_value(self.timeline_item_conversation(item_ref)?)?
            }
            ReadSelectionOperation::ArtifactExists { reference } => {
                serde_json::to_value(self.artifact_exists(reference)?)?
            }
            ReadSelectionOperation::ArtifactMessages { reference } => {
                serde_json::to_value(self.artifact_messages(reference)?)?
            }
            ReadSelectionOperation::ArtifactHtml { reference } => {
                serde_json::to_value(self.artifact_html(reference)?)?
            }
            ReadSelectionOperation::InvocationReplyRef { invocation_ref } => {
                serde_json::to_value(self.invocation_reply_ref(invocation_ref)?)?
            }
            ReadSelectionOperation::InvocationForkDestination {
                invocation_ref,
                intrinsic_actor_ref,
            } => serde_json::to_value(
                self.invocation_fork_destination(invocation_ref, intrinsic_actor_ref)?,
            )?,
            ReadSelectionOperation::InvocationStopFacts {
                invocation_ref,
                intrinsic_actor_ref,
            } => serde_json::to_value(
                self.invocation_stop_facts(invocation_ref, intrinsic_actor_ref)?,
            )?,
            ReadSelectionOperation::InvocationVisible {
                root_ref,
                invocation_ref,
            } => serde_json::to_value(self.invocation_visible(root_ref, invocation_ref)?)?,
            ReadSelectionOperation::StreamingMessageMetadata { message_ref } => {
                serde_json::to_value(self.streaming_message_metadata(message_ref)?)?
            }
            ReadSelectionOperation::MessageActionInvocation {
                conversation_ref,
                message_ref,
            } => serde_json::to_value(
                self.message_action_invocation(conversation_ref, message_ref)?,
            )?,
            ReadSelectionOperation::ProgressActivity {
                invocation_ref,
                source_activity_ref,
            } => {
                serde_json::to_value(self.progress_activity(invocation_ref, source_activity_ref)?)?
            }
            ReadSelectionOperation::ProgressInLatestAttempt {
                invocation_ref,
                sequence,
            } => serde_json::to_value(self.progress_in_latest_attempt(invocation_ref, *sequence)?)?,
            ReadSelectionOperation::LatestWorkActivity {
                invocation_ref,
                activity_ref,
            } => serde_json::to_value(self.latest_work_activity(invocation_ref, activity_ref)?)?,
            ReadSelectionOperation::TimelineRoot {
                conversation_ref,
                include_execution,
            } => serde_json::to_value(self.timeline_root(conversation_ref, *include_execution)?)?,
            ReadSelectionOperation::TimelineConnectionMetadata {
                conversation_ref,
                root_ref,
                connection_ref,
            } => serde_json::to_value(self.timeline_connection_metadata(
                conversation_ref,
                root_ref,
                connection_ref,
            )?)?,
            ReadSelectionOperation::TimelineCursorExists {
                connection_ref,
                sequence,
                item_ref,
            } => serde_json::to_value(self.timeline_cursor_exists(
                connection_ref,
                *sequence,
                item_ref,
            )?)?,
            ReadSelectionOperation::TimelineConnectionItems {
                connection_ref,
                lower,
                upper,
                limit,
                backwards,
            } => serde_json::to_value(self.timeline_connection_items(
                connection_ref,
                lower.unwrap_or(i64::MIN),
                upper.unwrap_or(i64::MAX),
                *limit,
                *backwards,
            )?)?,
            ReadSelectionOperation::TimelineConnectionCount {
                conversation_ref,
                connection_ref,
            } => serde_json::to_value(
                self.timeline_connection_count(conversation_ref, connection_ref)?,
            )?,
            ReadSelectionOperation::TimelineItem {
                conversation_ref,
                item_ref,
            } => serde_json::to_value(self.timeline_item(conversation_ref, item_ref)?)?,
            ReadSelectionOperation::InvocationTargetSource { invocation_ref } => {
                serde_json::to_value(self.invocation_target_source(invocation_ref)?)?
            }
            ReadSelectionOperation::MessageIsCommentary {
                message_ref,
                invocation_ref,
            } => serde_json::to_value(self.message_is_commentary(message_ref, invocation_ref)?)?,
            ReadSelectionOperation::MessageIsFinalReply {
                message_ref,
                target_actor_ref,
                source_message_ref,
            } => serde_json::to_value(self.message_is_final_reply(
                message_ref,
                target_actor_ref,
                source_message_ref.as_deref(),
            )?)?,
            ReadSelectionOperation::LastAddressedActors {
                conversation_ref,
                root_ref,
                author_actor_ref,
            } => serde_json::to_value(self.last_addressed_actor_refs(
                conversation_ref,
                root_ref,
                author_actor_ref,
            )?)?,
            _ => return Err("invalid finite selection dispatch".into()),
        })
    }
}
