use super::*;
impl ReadSelectionRequest {
    pub(crate) fn bounded_control(&self, host: &ReadControl) -> CwpdbResult<ReadControl> {
        host.check()?;
        native::valid_ref(&self.scope.runtime_root_ref)?;
        if self.max_bytes == 0 || self.max_bytes > MAX_WIRE_BYTES {
            return Err("invalid read selection byte budget".into());
        }
        if !matches!(self.selection, ReadSelectionOperation::Position)
            && self.expected_commit_revision.is_none()
        {
            return Err("conversation read snapshot revision conflict".into());
        }
        if serde_json::to_vec(self)?.len() > MAX_WIRE_BYTES + 65536 {
            return Err("declared read exceeds byte budget".into());
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
        let remaining = self
            .deadline_unix_ms
            .checked_sub(i64::try_from(now.as_millis())?)
            .filter(|value| *value > 0)
            .ok_or("conversation read operation exceeded its deadline")?;
        let mut control = host.clone();
        let request_deadline = std::time::Instant::now()
            .checked_add(std::time::Duration::from_millis(u64::try_from(remaining)?))
            .unwrap_or(control.deadline);
        control.deadline = control.deadline.min(request_deadline);
        Ok(control)
    }
    pub(crate) fn authorize(&self, authorizer: &dyn Authorizer) -> CwpdbResult<()> {
        authorizer.authorize(&self.scope.context, &self.scope.runtime_root_ref, false)?;
        let allow = |reference: &str| -> CwpdbResult<()> {
            native::valid_ref(reference)?;
            authorizer.authorize(&self.scope.context, reference, false)
        };
        self.selection.authorize_references(&allow)
    }
    pub(crate) fn authorize_result(
        &self,
        facts: &ReadSelectionFacts,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<()> {
        if matches!(self.selection, ReadSelectionOperation::Resource { .. }) {
            if let Some(reference) = facts.result.get("conversation_ref").and_then(Value::as_str) {
                authorizer.authorize(&self.scope.context, reference, false)?;
            }
        }
        if matches!(
            self.selection,
            ReadSelectionOperation::InspectorInvocationRoot { .. }
                | ReadSelectionOperation::TimelineItemConversation { .. }
        ) {
            if let Some(reference) = facts.result.as_str() {
                authorizer.authorize(&self.scope.context, reference, false)?;
            }
        }
        Ok(())
    }
}

impl ReadSelectionOperation {
    fn authorize_references(&self, allow: &dyn Fn(&str) -> CwpdbResult<()>) -> CwpdbResult<()> {
        match self {
            Self::Position
            | Self::ContentPositions { .. }
            | Self::CompletionPage { .. }
            | Self::TimelinePage { .. }
            | Self::LegacyTimelinePage { .. }
            | Self::Current { .. }
            | Self::ConversationExists { .. }
            | Self::RootConversationPresentation { .. }
            | Self::ForkSource { .. }
            | Self::OwnerContext { .. }
            | Self::TimelineRoot { .. }
            | Self::SummaryRow { .. }
            | Self::Resource { .. }
            | Self::ArtifactExists { .. }
            | Self::ArtifactMessages { .. }
            | Self::ArtifactHtml { .. }
            | Self::TimelineItemConversation { .. }
            | Self::ActorIdentity { .. }
            | Self::ActorCandidate { .. }
            | Self::ActorConversations { .. }
            | Self::ActorResources { .. }
            | Self::InvocationReplyRef { .. }
            | Self::ProgressInLatestAttempt { .. }
            | Self::LatestWorkActivity { .. }
            | Self::InspectorCaptureAvailable { .. }
            | Self::InspectorInvocationRoot { .. }
            | Self::InvocationTargetSource { .. }
            | Self::StreamingMessageMetadata { .. }
            | Self::TimelineConnectionItems { .. } => self.authorize_single(allow),
            Self::ContentMessage { .. }
            | Self::LegacyTimelineMessage { .. }
            | Self::SummaryMessageRecord { .. }
            | Self::MessageActionInvocation { .. }
            | Self::Invocation { .. }
            | Self::ParticipantVisible { .. }
            | Self::InvocationForkDestination { .. }
            | Self::InvocationStopFacts { .. }
            | Self::InvocationVisible { .. }
            | Self::ProgressActivity { .. }
            | Self::TimelineCursorExists { .. }
            | Self::TimelineConnectionCount { .. }
            | Self::TimelineItem { .. }
            | Self::InspectorDetailsAllowed { .. }
            | Self::MessageIsCommentary { .. } => self.authorize_paired(allow),
            Self::HistoryRead { .. }
            | Self::TimelineConnectionMetadata { .. }
            | Self::InspectorScope { .. }
            | Self::InspectorActivitySequence { .. }
            | Self::InspectorExitCode { .. }
            | Self::MessageIsFinalReply { .. }
            | Self::LastAddressedActors { .. }
            | Self::SummaryWindow { .. }
            | Self::InspectorPage { .. }
            | Self::InspectorText { .. } => self.authorize_compound(allow),
        }
    }
}
mod compound;
mod paired;
mod single;
