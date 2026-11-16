impl ConversationAdmissionStore {
    pub fn record_host_exchange(
        &mut self,
        conversation_ref: Option<&str>,
        requesting_actor_ref: &str,
        host_actor_ref: &str,
        request_text: String,
        response_text: String,
    ) -> CwrResult<HostConversationExchange> {
        for (label, actor_ref) in [
            ("host exchange requester", requesting_actor_ref),
            ("host exchange actor", host_actor_ref),
        ] {
            validate_ref(label, actor_ref)?;
        }
        if request_text.trim().is_empty() || response_text.trim().is_empty() {
            return invalid("host exchange request and response must not be blank");
        }

        let mut exchange = self.transact(|staged| {
            let creating = conversation_ref.is_none();
            let conversation_ref = conversation_ref
                .map(str::to_owned)
                .unwrap_or_else(|| new_ref(&staged.runtime_root_ref, "conversations"));
            let conversation = if let Some(conversation) = staged
                .conversations
                .find_mut(|conversation| conversation.conversation_ref == conversation_ref)
            {
                conversation
            } else if creating {
                staged.conversations.push(RetainedConversation {
                    owner_context_ref: None,
                    conversation_ref: conversation_ref.clone(),
                    title: None,
                    archived: false,
                    sidebar_eligible: Some(true),
                    parent_conversation_ref: None,
                    participant_actor_refs: Vec::new(),
                    messages: Vec::new(),
                    timeline_connection_ref: None,
                    activity: Vec::new(),
                });
                staged
                    .conversations
                    .last_mut()
                    .expect("conversation was appended")
            } else {
                return invalid(format!("unknown conversation ref {conversation_ref}"));
            };
            push_unique(
                &mut conversation.participant_actor_refs,
                requesting_actor_ref,
            );
            push_unique(&mut conversation.participant_actor_refs, host_actor_ref);

            let request_message_ref = new_ref(&staged.runtime_root_ref, "messages");
            conversation.record_message(ConversationMessage {
                file_refs: Vec::new(),
                progress: None,
                message_ref: request_message_ref.clone(),
                author_actor_ref: requesting_actor_ref.into(),
                addressed_actor_refs: vec![host_actor_ref.into()],
                addressing_mode: Some(ConversationAddressingMode::Explicit),
                text: request_text,
                visualizations: Vec::new(),
                reply_to_message_ref: None,
                action: None,
            });
            let response_message_ref = new_ref(&staged.runtime_root_ref, "messages");
            conversation.record_message(ConversationMessage {
                file_refs: Vec::new(),
                progress: None,
                message_ref: response_message_ref.clone(),
                author_actor_ref: host_actor_ref.into(),
                addressed_actor_refs: vec![requesting_actor_ref.into()],
                addressing_mode: Some(ConversationAddressingMode::Implicit),
                text: response_text,
                visualizations: Vec::new(),
                reply_to_message_ref: Some(request_message_ref.clone()),
                action: None,
            });
            let conversation = conversation.clone();

            Ok(HostConversationExchange {
                conversation,
                request_message_ref,
                response_message_ref,
            })
        })?;
        exchange.conversation = self
            .conversation(&exchange.conversation.conversation_ref)
            .cloned()
            .ok_or_else(|| invalid_error("committed host exchange conversation disappeared"))?;
        Ok(exchange)
    }

    pub fn thread_observation(&self, conversation_ref: &str) -> CwrResult<Value> {
        self.full_conversation(conversation_ref)?
            .as_ref()
            .map(conversation_thread)
            .ok_or_else(|| invalid_error(format!("unknown conversation ref {conversation_ref}")))
    }

    pub fn append_action_call(
        &mut self,
        conversation_ref: &str,
        caller_actor_ref: &str,
        target_actor_ref: &str,
        activation: &str,
        input: Value,
        text: String,
    ) -> CwrResult<ConversationMessage> {
        self.append_action_message(
            conversation_ref,
            caller_actor_ref,
            target_actor_ref,
            text,
            None,
            ConversationActionEvent::Call {
                activation: activation.into(),
                input,
            },
        )
    }

    pub fn append_host_action_call(
        &mut self,
        call: ConversationHostActionCall<'_>,
    ) -> CwrResult<ConversationMessage> {
        self.transact(|staged| {
            let message = push_action_message_in_state(
                staged,
                new_ref(&staged.runtime_root_ref, "messages"),
                call.conversation_ref,
                call.caller_actor_ref,
                call.target_actor_ref,
                call.text,
                None,
                ConversationActionEvent::Call {
                    activation: call.activation.into(),
                    input: call.input,
                },
            )?;
            invocation::push_execution_activity_in_state(
                staged,
                call.caller_invocation_ref,
                ConversationActivityEvent::HostActionStarted {
                    call_id: call.host_call_id.into(),
                    nested_invocation_ref: call.nested_invocation_ref.into(),
                    call_message_ref: message.message_ref.clone(),
                },
                current_unix_ms(),
            )?;

            Ok(message)
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "A retained action result records the full actor, activation, outcome, and reply identity."
    )]
    pub fn append_action_result(
        &mut self,
        conversation_ref: &str,
        target_actor_ref: &str,
        caller_actor_ref: &str,
        activation: &str,
        result: Value,
        outcome: ConversationActionOutcome,
        text: String,
        call_message_ref: &str,
    ) -> CwrResult<ConversationMessage> {
        self.append_action_message(
            conversation_ref,
            target_actor_ref,
            caller_actor_ref,
            text,
            Some(call_message_ref.into()),
            ConversationActionEvent::Result {
                activation: activation.into(),
                result,
                outcome: Some(outcome),
            },
        )
    }

    fn append_action_message(
        &mut self,
        conversation_ref: &str,
        author_actor_ref: &str,
        addressed_actor_ref: &str,
        text: String,
        reply_to_message_ref: Option<String>,
        action: ConversationActionEvent,
    ) -> CwrResult<ConversationMessage> {
        self.transact(|staged| {
            let message = push_action_message_in_state(
                staged,
                new_ref(&staged.runtime_root_ref, "messages"),
                conversation_ref,
                author_actor_ref,
                addressed_actor_ref,
                text,
                reply_to_message_ref,
                action,
            )?;

            Ok(message)
        })
    }

    pub fn admission(
        &self,
        integration_ref: &str,
        provider_event_ref: &str,
    ) -> Option<&ConversationAdmissionRecord> {
        self.find_admission(integration_ref, provider_event_ref)
    }

    fn find_admission(
        &self,
        integration_ref: &str,
        provider_event_ref: &str,
    ) -> Option<&ConversationAdmissionRecord> {
        self.state.admissions.iter().find(|record| {
            record.integration_ref == integration_ref
                && record.provider_event_ref == provider_event_ref
        })
    }

    fn new_ref(&self, collection: &str) -> String {
        new_ref(&self.state.runtime_root_ref, collection)
    }

    #[cfg(test)]
    fn replace_state(&mut self, state: ConversationRuntimeState) -> CwrResult<()> {
        state.invocation_ledger.validate()?;
        invocation::validate_invocation_execution_links(&state)?;
        timeline::validate_timeline_transition(&self.state, &state)?;
        visualization::validate_artifacts(&state)?;
        self.persist_state(&state)?;
        self.state = state;
        self.state.visualization_artifacts = Default::default();
        self.rebuild_admission_indices();
        Ok(())
    }

    #[cfg(test)]
    fn persist_state(&mut self, state: &ConversationRuntimeState) -> CwrResult<()> {
        let staged=self.state.fixture_diff(state)?;
        let mut backend=self.backend.lock().map_err(|_|invalid_error("conversation backend lock poisoned"))?;
        backend.set_expected_revision(self.commit_revision);
        backend.prepare(&self.state)?;
        self.commit_revision=backend.commit_fixture_state(&staged)?;
        let changed=staged.conversations.changes().filter(|(_,old,new)| *old != Some(*new)).map(|(_,_,new)|new.conversation_ref.clone()).collect::<Vec<_>>();
        crate::cwpdb::test_support::conversation::legacy_conversation_clocks(&self.state_path,&state.runtime_root_ref,self.commit_revision,&changed)?;
        Ok(())
    }
}
