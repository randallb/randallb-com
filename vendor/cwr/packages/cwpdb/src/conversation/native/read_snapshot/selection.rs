//! Dispatch only declared finite operations inside an already frozen native view.
use super::*;
impl NativeReadSnapshot {
    pub fn read_selection(
        &self,
        request: &ReadSelectionRequest,
    ) -> CwpdbResult<ReadSelectionFacts> {
        let bounded = request.bounded_control(&self.control)?;
        configure(&self.connection, &bounded)?;
        let _deadline = SelectionDeadline {
            connection: &self.connection,
            restore: &self.control,
        };
        if request.scope.runtime_root_ref != self.scope.runtime_root_ref
            || request.scope.context.workspace_ref != self.scope.context.workspace_ref
        {
            return Err("conversation read selection scope mismatch".into());
        }
        if request.max_bytes == 0 || request.max_bytes > MAX_WIRE_BYTES {
            return Err("invalid read selection byte budget".into());
        }
        let now = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis(),
        )?;
        if request.deadline_unix_ms <= now {
            return Err("conversation read operation exceeded its deadline".into());
        }
        if (!matches!(request.selection, ReadSelectionOperation::Position)
            && request.expected_commit_revision.is_none())
            || request
                .expected_commit_revision
                .as_deref()
                .is_some_and(|revision| revision != self.commit_revision)
        {
            return Err("conversation read snapshot revision conflict".into());
        }
        let result = self.selection_facts(&request.selection)?;
        bounded.check()?;
        let result = ReadSelectionFacts {
            runtime_root_ref: self.scope.runtime_root_ref.clone(),
            commit_revision: self.commit_revision.clone(),
            result,
        };
        if serde_json::to_vec(&result)?.len() > request.max_bytes {
            return Err("read selection exceeds response byte budget".into());
        }
        bounded.check()?;
        Ok(result)
    }
}

impl NativeReadSnapshot {
    fn selection_facts(&self, operation: &ReadSelectionOperation) -> CwpdbResult<Value> {
        match operation {
            ReadSelectionOperation::ContentMessage { .. }
            | ReadSelectionOperation::ContentPositions { .. }
            | ReadSelectionOperation::CompletionPage { .. }
            | ReadSelectionOperation::TimelinePage { .. }
            | ReadSelectionOperation::LegacyTimelinePage { .. }
            | ReadSelectionOperation::LegacyTimelineMessage { .. }
            | ReadSelectionOperation::SummaryWindow { .. }
            | ReadSelectionOperation::SummaryRow { .. }
            | ReadSelectionOperation::SummaryMessageRecord { .. }
            | ReadSelectionOperation::HistoryRead { .. } => self.select_pages(operation),
            ReadSelectionOperation::Position
            | ReadSelectionOperation::Current { .. }
            | ReadSelectionOperation::Invocation { .. }
            | ReadSelectionOperation::Resource { .. }
            | ReadSelectionOperation::ConversationExists { .. }
            | ReadSelectionOperation::ParticipantVisible { .. }
            | ReadSelectionOperation::RootConversationPresentation { .. }
            | ReadSelectionOperation::ForkSource { .. }
            | ReadSelectionOperation::OwnerContext { .. }
            | ReadSelectionOperation::ActorIdentity { .. }
            | ReadSelectionOperation::ActorCandidate { .. }
            | ReadSelectionOperation::ActorConversations { .. }
            | ReadSelectionOperation::ActorResources { .. } => self.select_current(operation),
            ReadSelectionOperation::TimelineItemConversation { .. }
            | ReadSelectionOperation::ArtifactExists { .. }
            | ReadSelectionOperation::ArtifactMessages { .. }
            | ReadSelectionOperation::ArtifactHtml { .. }
            | ReadSelectionOperation::InvocationReplyRef { .. }
            | ReadSelectionOperation::InvocationForkDestination { .. }
            | ReadSelectionOperation::InvocationStopFacts { .. }
            | ReadSelectionOperation::InvocationVisible { .. }
            | ReadSelectionOperation::StreamingMessageMetadata { .. }
            | ReadSelectionOperation::MessageActionInvocation { .. }
            | ReadSelectionOperation::ProgressActivity { .. }
            | ReadSelectionOperation::ProgressInLatestAttempt { .. }
            | ReadSelectionOperation::LatestWorkActivity { .. }
            | ReadSelectionOperation::TimelineRoot { .. }
            | ReadSelectionOperation::TimelineConnectionMetadata { .. }
            | ReadSelectionOperation::TimelineCursorExists { .. }
            | ReadSelectionOperation::TimelineConnectionItems { .. }
            | ReadSelectionOperation::TimelineConnectionCount { .. }
            | ReadSelectionOperation::TimelineItem { .. }
            | ReadSelectionOperation::InvocationTargetSource { .. }
            | ReadSelectionOperation::MessageIsCommentary { .. }
            | ReadSelectionOperation::MessageIsFinalReply { .. }
            | ReadSelectionOperation::LastAddressedActors { .. } => {
                self.select_publication(operation)
            }
            ReadSelectionOperation::InspectorCaptureAvailable { .. }
            | ReadSelectionOperation::InspectorDetailsAllowed { .. }
            | ReadSelectionOperation::InspectorInvocationRoot { .. }
            | ReadSelectionOperation::InspectorScope { .. }
            | ReadSelectionOperation::InspectorPage { .. }
            | ReadSelectionOperation::InspectorActivitySequence { .. }
            | ReadSelectionOperation::InspectorExitCode { .. }
            | ReadSelectionOperation::InspectorText { .. } => self.select_inspector(operation),
        }
    }
}
mod current;
mod inspector;
mod pages;
mod publication;
