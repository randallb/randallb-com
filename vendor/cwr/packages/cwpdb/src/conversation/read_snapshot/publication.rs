//! Typed finite selection methods for native and remote backends.
use super::*;
impl ReadSnapshot {
    pub fn participant_visible(&self, conversation: &str, actor: &str) -> CwpdbResult<bool> {
        match self {
            Self::Native(snapshot) => snapshot.participant_visible(conversation, actor),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ParticipantVisible {
                conversation_ref: conversation.into(),
                actor_ref: actor.into(),
            }),
        }
    }
    pub fn root_conversation_presentation(
        &self,
        conversation: &str,
    ) -> CwpdbResult<Option<(Option<String>, bool)>> {
        match self {
            Self::Native(snapshot) => snapshot.root_conversation_presentation(conversation),
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::RootConversationPresentation {
                    conversation_ref: conversation.into(),
                })
            }
        }
    }
    pub fn fork_source(&self, conversation: &str) -> CwpdbResult<Option<(String, String)>> {
        match self {
            Self::Native(snapshot) => snapshot.fork_source(conversation),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ForkSource {
                conversation_ref: conversation.into(),
            }),
        }
    }
    pub fn owner_context_ref(&self, conversation: &str) -> CwpdbResult<Option<String>> {
        match self {
            Self::Native(snapshot) => snapshot.owner_context_ref(conversation),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::OwnerContext {
                conversation_ref: conversation.into(),
            }),
        }
    }
    pub fn timeline_item_conversation(&self, item: &str) -> CwpdbResult<Option<String>> {
        match self {
            Self::Native(snapshot) => snapshot.timeline_item_conversation(item),
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::TimelineItemConversation {
                    item_ref: item.into(),
                })
            }
        }
    }
    pub fn actor_identity(&self, actor: &str) -> CwpdbResult<Option<Value>> {
        match self {
            Self::Native(snapshot) => snapshot.actor_identity(actor),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ActorIdentity {
                actor_ref: actor.into(),
            }),
        }
    }
    pub fn actor_candidate(&self, actor: &str) -> CwpdbResult<bool> {
        match self {
            Self::Native(snapshot) => snapshot.actor_candidate(actor),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ActorCandidate {
                actor_ref: actor.into(),
            }),
        }
    }
    pub fn actor_conversations(&self, actor: &str) -> CwpdbResult<Vec<String>> {
        match self {
            Self::Native(snapshot) => snapshot.actor_conversations(actor),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ActorConversations {
                actor_ref: actor.into(),
            }),
        }
    }
    pub fn actor_resources(&self, actor: &str) -> CwpdbResult<Vec<String>> {
        match self {
            Self::Native(snapshot) => snapshot.actor_resources(actor),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ActorResources {
                actor_ref: actor.into(),
            }),
        }
    }
    pub fn artifact_exists(&self, reference: &str) -> CwpdbResult<bool> {
        match self {
            Self::Native(snapshot) => snapshot.artifact_exists(reference),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ArtifactExists {
                reference: reference.into(),
            }),
        }
    }
    pub fn artifact_messages(&self, reference: &str) -> CwpdbResult<Vec<String>> {
        match self {
            Self::Native(snapshot) => snapshot.artifact_messages(reference),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ArtifactMessages {
                reference: reference.into(),
            }),
        }
    }
    pub fn artifact_html(&self, reference: &str) -> CwpdbResult<Option<String>> {
        match self {
            Self::Native(snapshot) => snapshot.artifact_html(reference),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::ArtifactHtml {
                reference: reference.into(),
            }),
        }
    }
    pub fn invocation_reply_ref(&self, invocation: &str) -> CwpdbResult<Option<String>> {
        match self {
            Self::Native(snapshot) => snapshot.invocation_reply_ref(invocation),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::InvocationReplyRef {
                invocation_ref: invocation.into(),
            }),
        }
    }
    pub fn invocation_fork_destination(
        &self,
        invocation: &str,
        intrinsic: &str,
    ) -> CwpdbResult<Option<String>> {
        match self {
            Self::Native(snapshot) => snapshot.invocation_fork_destination(invocation, intrinsic),
            Self::Remote(snapshot) => {
                snapshot.read(ReadSelectionOperation::InvocationForkDestination {
                    invocation_ref: invocation.into(),
                    intrinsic_actor_ref: intrinsic.into(),
                })
            }
        }
    }
    pub fn invocation_stop_facts(
        &self,
        invocation: &str,
        intrinsic: &str,
    ) -> CwpdbResult<Option<InvocationStopFacts>> {
        match self {
            Self::Native(snapshot) => snapshot.invocation_stop_facts(invocation, intrinsic),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::InvocationStopFacts {
                invocation_ref: invocation.into(),
                intrinsic_actor_ref: intrinsic.into(),
            }),
        }
    }
    pub fn invocation_visible(&self, root: &str, invocation: &str) -> CwpdbResult<bool> {
        match self {
            Self::Native(snapshot) => snapshot.invocation_visible(root, invocation),
            Self::Remote(snapshot) => snapshot.read(ReadSelectionOperation::InvocationVisible {
                root_ref: root.into(),
                invocation_ref: invocation.into(),
            }),
        }
    }
}
