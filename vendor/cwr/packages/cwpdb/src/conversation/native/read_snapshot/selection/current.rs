use super::*;
impl NativeReadSnapshot {
    pub(super) fn select_current(&self, operation: &ReadSelectionOperation) -> CwpdbResult<Value> {
        Ok(match operation {
            ReadSelectionOperation::Position => serde_json::Value::Null,
            ReadSelectionOperation::Current { conversation_ref } => {
                serde_json::to_value(self.current(conversation_ref)?)?
            }
            ReadSelectionOperation::Invocation {
                conversation_ref,
                invocation_ref,
            } => serde_json::to_value(self.invocation(conversation_ref, invocation_ref)?)?,
            ReadSelectionOperation::Resource { reference } => {
                serde_json::to_value(self.resource(reference)?)?
            }
            ReadSelectionOperation::ConversationExists {
                conversation_ref,
                include_execution,
                owner,
            } => serde_json::to_value(
                self.conversation_exists(
                    conversation_ref,
                    *include_execution,
                    owner
                        .as_ref()
                        .map(|(owner, legacy)| (owner.as_str(), *legacy)),
                )?,
            )?,
            ReadSelectionOperation::ParticipantVisible {
                conversation_ref,
                actor_ref,
            } => serde_json::to_value(self.participant_visible(conversation_ref, actor_ref)?)?,
            ReadSelectionOperation::RootConversationPresentation { conversation_ref } => {
                serde_json::to_value(self.root_conversation_presentation(conversation_ref)?)?
            }
            ReadSelectionOperation::ForkSource { conversation_ref } => {
                serde_json::to_value(self.fork_source(conversation_ref)?)?
            }
            ReadSelectionOperation::OwnerContext { conversation_ref } => {
                serde_json::to_value(self.owner_context_ref(conversation_ref)?)?
            }
            ReadSelectionOperation::ActorIdentity { actor_ref } => {
                serde_json::to_value(self.actor_identity(actor_ref)?)?
            }
            ReadSelectionOperation::ActorCandidate { actor_ref } => {
                serde_json::to_value(self.actor_candidate(actor_ref)?)?
            }
            ReadSelectionOperation::ActorConversations { actor_ref } => {
                serde_json::to_value(self.actor_conversations(actor_ref)?)?
            }
            ReadSelectionOperation::ActorResources { actor_ref } => {
                serde_json::to_value(self.actor_resources(actor_ref)?)?
            }
            _ => return Err("invalid finite selection dispatch".into()),
        })
    }
}
