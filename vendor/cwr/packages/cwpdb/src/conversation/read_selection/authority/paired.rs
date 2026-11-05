use super::*;
impl ReadSelectionOperation {
    pub(super) fn authorize_paired(
        &self,
        allow: &dyn Fn(&str) -> CwpdbResult<()>,
    ) -> CwpdbResult<()> {
        match self {
            Self::ContentMessage {
                conversation_ref,
                message_ref,
                ..
            }
            | Self::LegacyTimelineMessage {
                conversation_ref,
                message_ref,
                ..
            }
            | Self::SummaryMessageRecord {
                conversation_ref,
                message_ref,
                ..
            }
            | Self::MessageActionInvocation {
                conversation_ref,
                message_ref,
                ..
            } => {
                allow(conversation_ref)?;
                allow(message_ref)?;
            }
            Self::Invocation {
                conversation_ref,
                invocation_ref,
                ..
            } => {
                allow(conversation_ref)?;
                allow(invocation_ref)?;
            }
            Self::ParticipantVisible {
                conversation_ref,
                actor_ref,
                ..
            } => {
                allow(conversation_ref)?;
                allow(actor_ref)?;
            }
            Self::InvocationForkDestination {
                invocation_ref,
                intrinsic_actor_ref,
                ..
            }
            | Self::InvocationStopFacts {
                invocation_ref,
                intrinsic_actor_ref,
                ..
            } => {
                allow(invocation_ref)?;
                allow(intrinsic_actor_ref)?;
            }
            Self::InvocationVisible {
                root_ref,
                invocation_ref,
                ..
            } => {
                allow(root_ref)?;
                allow(invocation_ref)?;
            }
            Self::ProgressActivity {
                invocation_ref,
                source_activity_ref,
                ..
            } => {
                allow(invocation_ref)?;
                allow(source_activity_ref)?;
            }
            Self::TimelineCursorExists {
                connection_ref,
                item_ref,
                ..
            } => {
                allow(connection_ref)?;
                allow(item_ref)?;
            }
            Self::TimelineConnectionCount {
                conversation_ref,
                connection_ref,
                ..
            } => {
                allow(conversation_ref)?;
                allow(connection_ref)?;
            }
            Self::TimelineItem {
                conversation_ref,
                item_ref,
                ..
            } => {
                allow(conversation_ref)?;
                allow(item_ref)?;
            }
            Self::InspectorDetailsAllowed {
                invocation_ref,
                actor_ref,
                ..
            } => {
                allow(invocation_ref)?;
                allow(actor_ref)?;
            }
            Self::MessageIsCommentary {
                message_ref,
                invocation_ref,
                ..
            } => {
                allow(message_ref)?;
                allow(invocation_ref)?;
            }
            _ => return Err("invalid finite authority dispatch".into()),
        }
        Ok(())
    }
}
