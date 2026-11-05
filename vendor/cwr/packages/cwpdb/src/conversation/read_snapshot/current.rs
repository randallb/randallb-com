//! Typed finite selection methods for native and remote backends.
use super::*;
impl ReadSnapshot {
    pub fn current(&self, conversation: &str) -> CwpdbResult<Vec<ResourceRecordFacts>> {
        match self {
            Self::Native(snapshot) => snapshot.current(conversation),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::Current {
                conversation_ref: conversation.into(),
            }),
        }
    }
    pub fn invocation(
        &self,
        conversation: &str,
        invocation: &str,
    ) -> CwpdbResult<Option<ResourceRecordFacts>> {
        match self {
            Self::Native(snapshot) => snapshot.invocation(conversation, invocation),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::Invocation {
                conversation_ref: conversation.into(),
                invocation_ref: invocation.into(),
            }),
        }
    }
    pub fn resource(&self, reference: &str) -> CwpdbResult<Option<ResourceFacts>> {
        match self {
            Self::Native(snapshot) => snapshot.resource(reference),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::Resource {
                reference: reference.into(),
            }),
        }
    }
    pub fn conversation_exists(
        &self,
        conversation: &str,
        include_execution: bool,
        owner: Option<(&str, bool)>,
    ) -> CwpdbResult<bool> {
        match self {
            Self::Native(snapshot) => {
                snapshot.conversation_exists(conversation, include_execution, owner)
            }
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ConversationExists {
                conversation_ref: conversation.into(),
                include_execution,
                owner: owner.map(|(owner, legacy)| (owner.into(), legacy)),
            }),
        }
    }
    pub fn history_read(
        &self,
        reference: &str,
        selection: HistorySelection,
        participant: Option<String>,
        include_execution: bool,
    ) -> CwpdbResult<Option<HistoryReadFacts>> {
        match self {
            Self::Native(snapshot) => snapshot.history_read(
                reference,
                selection.clone(),
                participant.clone(),
                include_execution,
            ),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::HistoryRead {
                conversation_ref: reference.into(),
                selection,
                participant_actor_ref: participant,
                include_execution,
            }),
        }
    }
}
