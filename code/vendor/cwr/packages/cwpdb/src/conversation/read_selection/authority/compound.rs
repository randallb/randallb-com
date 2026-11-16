use super::*;
impl ReadSelectionOperation {
    pub(super) fn authorize_compound(
        &self,
        allow: &dyn Fn(&str) -> CwpdbResult<()>,
    ) -> CwpdbResult<()> {
        match self {
            Self::HistoryRead {
                conversation_ref,
                participant_actor_ref,
                ..
            } => {
                allow(conversation_ref)?;
                if let Some(reference) = participant_actor_ref {
                    allow(reference)?;
                }
            }
            Self::TimelineConnectionMetadata {
                conversation_ref,
                root_ref,
                connection_ref,
                ..
            } => {
                allow(conversation_ref)?;
                allow(root_ref)?;
                allow(connection_ref)?;
            }
            Self::InspectorScope {
                actor_ref,
                invocation_ref,
                activity_ref,
                ..
            } => {
                allow(actor_ref)?;
                allow(invocation_ref)?;
                if let Some(reference) = activity_ref {
                    allow(reference)?;
                }
            }
            Self::InspectorActivitySequence {
                invocation_ref,
                conversation_ref,
                activity_ref,
                ..
            } => {
                allow(invocation_ref)?;
                allow(conversation_ref)?;
                allow(activity_ref)?;
            }
            Self::InspectorExitCode {
                invocation_ref,
                conversation_ref,
                actor_ref,
                activity_ref,
                ..
            } => {
                allow(invocation_ref)?;
                allow(conversation_ref)?;
                allow(actor_ref)?;
                allow(activity_ref)?;
            }
            Self::MessageIsFinalReply {
                message_ref,
                target_actor_ref,
                source_message_ref,
                ..
            } => {
                allow(message_ref)?;
                allow(target_actor_ref)?;
                if let Some(reference) = source_message_ref {
                    allow(reference)?;
                }
            }
            Self::LastAddressedActors {
                conversation_ref,
                root_ref,
                author_actor_ref,
                ..
            } => {
                allow(conversation_ref)?;
                allow(root_ref)?;
                allow(author_actor_ref)?;
            }
            Self::SummaryWindow { request } => {
                if let Some(actor) = &request.participant_actor_ref {
                    allow(actor)?;
                }
            }
            Self::InspectorPage { request } => {
                for reference in [
                    &request.invocation_ref,
                    &request.conversation_ref,
                    &request.actor_ref,
                ] {
                    allow(reference)?;
                }
            }
            Self::InspectorText { request } => {
                for reference in [
                    &request.invocation_ref,
                    &request.conversation_ref,
                    &request.actor_ref,
                ] {
                    allow(reference)?;
                }
            }
            _ => return Err("invalid finite authority dispatch".into()),
        }
        Ok(())
    }
}
