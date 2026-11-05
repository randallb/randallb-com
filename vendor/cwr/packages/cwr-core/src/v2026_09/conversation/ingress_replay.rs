impl ConversationAdmissionStore {
    /// Recover an action using retained routing, without consulting a live activation.
    pub(crate) fn recover_action(
        &self,
        ingress: &ConversationIngress,
    ) -> CwrResult<Option<ConversationAdmissionRecord>> {
        Ok(self
            .replayed_admission(ingress, &conversation_ingress_fingerprint(ingress)?)?
            .map(|r| *r))
    }

    fn validate_admission_replay(
        &self,
        ingress: &ConversationIngress,
        fingerprint: &str,
        record: &ConversationAdmissionRecord,
    ) -> CwrResult<()> {
        let matches = match record.ingress_fingerprint.as_deref() {
            None => self.legacy_admission_matches(ingress, record),
            Some(existing) if existing == fingerprint => true,
            Some(existing)
                if matches!(
                    ingress.conversation,
                    ConversationIngressConversation::CreateForAction
                ) && matches!(
                    ingress.addressing,
                    ConversationIngressAddressing::Action { .. }
                ) =>
            {
                // SubmitAction used Create before its origin was recorded separately.
                // Only normalize that representation; every other ingress field must match.
                let mut previous = ingress.clone();
                previous.conversation = ConversationIngressConversation::Create;
                existing == conversation_ingress_fingerprint(&previous)?
            }
            Some(_) => false,
        };
        if matches {
            return Ok(());
        }
        invalid(format!(
            "conversation ingress event {} was reused with different input",
            ingress.provider_event_ref
        ))
    }

    fn replayed_admission(
        &self,
        ingress: &ConversationIngress,
        fingerprint: &str,
    ) -> CwrResult<Option<Box<ConversationAdmissionRecord>>> {
        let Some(record) =
            self.find_admission(&ingress.integration_ref, &ingress.provider_event_ref)
        else {
            return Ok(None);
        };
        validate_owner_context(ingress, &self.ingress_conversation(ingress)?)?;
        self.validate_admission_replay(ingress, fingerprint, record)?;
        Ok(Some(Box::new(record.clone())))
    }

    fn legacy_admission_matches(
        &self,
        ingress: &ConversationIngress,
        record: &ConversationAdmissionRecord,
    ) -> bool {
        let Some(conversation) = self
            .state
            .conversations
            .iter()
            .find(|conversation| conversation.conversation_ref == record.conversation_ref)
        else {
            return false;
        };
        let Some((message_index, message)) = conversation
            .messages
            .iter()
            .enumerate()
            .find(|(_, message)| message.message_ref == record.source_message_ref)
        else {
            return false;
        };
        if message.author_actor_ref != ingress.author_actor_ref
            || message.text != ingress.text
            || message.file_refs != ingress.file_refs
        {
            return false;
        }
        let conversation_matches = match &ingress.conversation {
            ConversationIngressConversation::Create
            | ConversationIngressConversation::CreateForAction => {
                message_index == 0 && ingress.owner_context_ref == conversation.owner_context_ref
            },
            ConversationIngressConversation::Continue { conversation_ref } => {
                conversation_ref == &record.conversation_ref && message_index > 0
            }
        };
        if !conversation_matches {
            return false;
        }
        match &ingress.addressing {
            ConversationIngressAddressing::Addressed { actor_refs } => {
                message.action.is_none() && message.addressed_actor_refs == *actor_refs
            }
            ConversationIngressAddressing::Action {
                actor_ref,
                activation,
                action_name,
                input,
            } => {
                message.addressed_actor_refs.len() == 1
                    && message.addressed_actor_refs.first() == Some(actor_ref)
                    && matches!(
                        &message.action,
                        Some(ConversationActionEvent::Call {
                            activation: retained_activation,
                            input: retained_input,
                        }) if retained_activation == activation && retained_input == input
                    )
                    && record
                        .invocation_ref
                        .as_deref()
                        .and_then(|invocation_ref| self.invocation_record(invocation_ref))
                        .is_some_and(|invocation| invocation.action_name == *action_name)
            }
            ConversationIngressAddressing::Unaddressed => message.action.is_none(),
            ConversationIngressAddressing::UnresolvedLeadingHandle { .. } => false,
        }
    }

}
