//! Typed finite selection methods for native and remote backends.
use super::*;
impl ReadSnapshot {
    pub fn streaming_message_metadata(
        &self,
        message: &str,
    ) -> CwpdbResult<(Option<String>, String)> {
        match self {
            Self::Native(snapshot) => snapshot.streaming_message_metadata(message),
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::StreamingMessageMetadata {
                    message_ref: message.into(),
                })
            }
        }
    }
    pub fn message_action_invocation(
        &self,
        conversation: &str,
        message: &str,
    ) -> CwpdbResult<Option<String>> {
        match self {
            Self::Native(snapshot) => snapshot.message_action_invocation(conversation, message),
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::MessageActionInvocation {
                    conversation_ref: conversation.into(),
                    message_ref: message.into(),
                })
            }
        }
    }
    pub fn progress_activity(&self, invocation: &str, source: &str) -> CwpdbResult<Option<Value>> {
        match self {
            Self::Native(snapshot) => snapshot.progress_activity(invocation, source),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ProgressActivity {
                invocation_ref: invocation.into(),
                source_activity_ref: source.into(),
            }),
        }
    }
    pub fn progress_in_latest_attempt(
        &self,
        invocation: &str,
        sequence: Option<u64>,
    ) -> CwpdbResult<bool> {
        match self {
            Self::Native(snapshot) => snapshot.progress_in_latest_attempt(invocation, sequence),
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::ProgressInLatestAttempt {
                    invocation_ref: invocation.into(),
                    sequence,
                })
            }
        }
    }
    pub fn latest_work_activity(
        &self,
        invocation: &str,
        activity: &str,
    ) -> CwpdbResult<Option<Value>> {
        match self {
            Self::Native(snapshot) => snapshot.latest_work_activity(invocation, activity),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::LatestWorkActivity {
                invocation_ref: invocation.into(),
                activity_ref: activity.into(),
            }),
        }
    }
    pub fn invocation_target_source(
        &self,
        invocation: &str,
    ) -> CwpdbResult<Option<(String, Option<String>)>> {
        match self {
            Self::Native(snapshot) => snapshot.invocation_target_source(invocation),
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::InvocationTargetSource {
                    invocation_ref: invocation.into(),
                })
            }
        }
    }
    pub fn message_is_commentary(&self, message: &str, invocation: &str) -> CwpdbResult<bool> {
        match self {
            Self::Native(snapshot) => snapshot.message_is_commentary(message, invocation),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::MessageIsCommentary {
                message_ref: message.into(),
                invocation_ref: invocation.into(),
            }),
        }
    }
    pub fn message_is_final_reply(
        &self,
        message: &str,
        target: &str,
        source: Option<&str>,
    ) -> CwpdbResult<bool> {
        match self {
            Self::Native(snapshot) => snapshot.message_is_final_reply(message, target, source),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::MessageIsFinalReply {
                message_ref: message.into(),
                target_actor_ref: target.into(),
                source_message_ref: source.map(str::to_owned),
            }),
        }
    }
    pub fn last_addressed_actor_refs(
        &self,
        conversation: &str,
        root: &str,
        author: &str,
    ) -> CwpdbResult<Vec<String>> {
        match self {
            Self::Native(snapshot) => {
                snapshot.last_addressed_actor_refs(conversation, root, author)
            }
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::LastAddressedActors {
                conversation_ref: conversation.into(),
                root_ref: root.into(),
                author_actor_ref: author.into(),
            }),
        }
    }
}
