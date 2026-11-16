use super::*;
impl ReadSelectionOperation {
    pub(super) fn authorize_single(
        &self,
        allow: &dyn Fn(&str) -> CwpdbResult<()>,
    ) -> CwpdbResult<()> {
        match self {
            Self::Position => {}
            Self::ContentPositions {
                conversation_ref, ..
            }
            | Self::CompletionPage {
                conversation_ref, ..
            }
            | Self::TimelinePage {
                conversation_ref, ..
            }
            | Self::LegacyTimelinePage {
                conversation_ref, ..
            }
            | Self::Current {
                conversation_ref, ..
            }
            | Self::ConversationExists {
                conversation_ref, ..
            }
            | Self::RootConversationPresentation {
                conversation_ref, ..
            }
            | Self::ForkSource {
                conversation_ref, ..
            }
            | Self::OwnerContext {
                conversation_ref, ..
            }
            | Self::TimelineRoot {
                conversation_ref, ..
            } => {
                allow(conversation_ref)?;
            }
            Self::SummaryRow { reference, .. }
            | Self::Resource { reference, .. }
            | Self::ArtifactExists { reference, .. }
            | Self::ArtifactMessages { reference, .. }
            | Self::ArtifactHtml { reference, .. } => {
                allow(reference)?;
            }
            Self::TimelineItemConversation { item_ref, .. } => {
                allow(item_ref)?;
            }
            Self::ActorIdentity { actor_ref, .. }
            | Self::ActorCandidate { actor_ref, .. }
            | Self::ActorConversations { actor_ref, .. }
            | Self::ActorResources { actor_ref, .. } => {
                allow(actor_ref)?;
            }
            Self::InvocationReplyRef { invocation_ref, .. }
            | Self::ProgressInLatestAttempt { invocation_ref, .. }
            | Self::LatestWorkActivity { invocation_ref, .. }
            | Self::InspectorCaptureAvailable { invocation_ref, .. }
            | Self::InspectorInvocationRoot { invocation_ref, .. }
            | Self::InvocationTargetSource { invocation_ref, .. } => {
                allow(invocation_ref)?;
            }
            Self::StreamingMessageMetadata { message_ref, .. } => {
                allow(message_ref)?;
            }
            Self::TimelineConnectionItems { connection_ref, .. } => {
                allow(connection_ref)?;
            }
            _ => return Err("invalid finite authority dispatch".into()),
        }
        Ok(())
    }
}
