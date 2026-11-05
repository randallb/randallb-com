//! Typed finite selection methods for native and remote backends.
use super::*;
impl ReadSnapshot {
    pub fn summary_window(
        &self,
        request: &SummaryWindowRequest,
    ) -> CwpdbResult<SummaryWindowFacts> {
        match self {
            Self::Native(snapshot) => snapshot.summary_window(request),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::SummaryWindow {
                request: request.clone(),
            }),
        }
    }
    pub fn summary_row(
        &self,
        reference: &str,
        workspace: Option<(Option<&str>, &str)>,
    ) -> CwpdbResult<Option<SummaryWindowRow>> {
        match self {
            Self::Native(snapshot) => snapshot.summary_row(reference, workspace),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::SummaryRow {
                reference: reference.into(),
                workspace: workspace.map(|(owner, actor)| (owner.map(str::to_owned), actor.into())),
            }),
        }
    }
    pub fn summary_message_record(&self, conversation: &str, message: &str) -> CwpdbResult<Value> {
        match self {
            Self::Native(snapshot) => snapshot.summary_message_record(conversation, message),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::SummaryMessageRecord {
                conversation_ref: conversation.into(),
                message_ref: message.into(),
            }),
        }
    }
}
