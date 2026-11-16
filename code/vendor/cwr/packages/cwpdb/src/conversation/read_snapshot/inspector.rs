//! Typed finite selection methods for native and remote backends.
use super::*;
impl ReadSnapshot {
    pub fn inspector_capture_available(&self, invocation: &str) -> CwpdbResult<bool> {
        match self {
            Self::Native(snapshot) => snapshot.inspector_capture_available(invocation),
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::InspectorCaptureAvailable {
                    invocation_ref: invocation.into(),
                })
            }
        }
    }
    pub fn inspector_details_allowed(&self, invocation: &str, actor: &str) -> CwpdbResult<bool> {
        match self {
            Self::Native(snapshot) => snapshot.inspector_details_allowed(invocation, actor),
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::InspectorDetailsAllowed {
                    invocation_ref: invocation.into(),
                    actor_ref: actor.into(),
                })
            }
        }
    }
    pub fn inspector_invocation_root(&self, invocation: &str) -> CwpdbResult<Option<String>> {
        match self {
            Self::Native(snapshot) => snapshot.inspector_invocation_root(invocation),
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::InspectorInvocationRoot {
                    invocation_ref: invocation.into(),
                })
            }
        }
    }
    pub fn inspector_page(
        &self,
        request: &InspectorReadRequest,
    ) -> CwpdbResult<Vec<InspectorEntryFacts>> {
        match self {
            Self::Native(snapshot) => snapshot.inspector_page(request),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::InspectorPage {
                request: request.clone(),
            }),
        }
    }
    pub fn inspector_activity_sequence(
        &self,
        invocation: &str,
        conversation: &str,
        activity: &str,
    ) -> CwpdbResult<Option<i64>> {
        match self {
            Self::Native(snapshot) => {
                snapshot.inspector_activity_sequence(invocation, conversation, activity)
            }
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::InspectorActivitySequence {
                    invocation_ref: invocation.into(),
                    conversation_ref: conversation.into(),
                    activity_ref: activity.into(),
                })
            }
        }
    }
    pub fn inspector_exit_code(
        &self,
        invocation: &str,
        conversation: &str,
        actor: &str,
        activity: &str,
    ) -> CwpdbResult<Option<i64>> {
        match self {
            Self::Native(snapshot) => {
                snapshot.inspector_exit_code(invocation, conversation, actor, activity)
            }
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::InspectorExitCode {
                invocation_ref: invocation.into(),
                conversation_ref: conversation.into(),
                actor_ref: actor.into(),
                activity_ref: activity.into(),
            }),
        }
    }
    pub fn inspector_text(
        &self,
        request: &InspectorTextRequest,
    ) -> CwpdbResult<Option<InspectorTextFacts>> {
        match self {
            Self::Native(snapshot) => snapshot.inspector_text(request),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::InspectorText {
                request: request.clone(),
            }),
        }
    }
}
