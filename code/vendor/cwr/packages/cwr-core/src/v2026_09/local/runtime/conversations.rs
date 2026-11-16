impl LocalRuntime {
    /// Inspect a durable ingress receipt without admitting or executing work.
    pub fn admission_record(
        &self,
        integration_ref: &str,
        event_ref: &str,
    ) -> CwrResult<Option<ConversationAdmissionRecord>> {
        Ok(self
            .lock()?
            .admission
            .admission(integration_ref, event_ref)
            .cloned())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Recovery validates the original action and the current installed owner disclosure scope."
    )]
    pub(super) fn recover_action_request(
        &self,
        integration_ref: &str,
        request_id: &str,
        author_actor_ref: &str,
        conversation_ref: Option<&str>,
        activation: &str,
        input: &Value,
        owner_context_ref: Option<&str>,
    ) -> CwrResult<Option<LocalAdmission>> {
        let state = self.lock()?;
        if let Some(owner) = owner_context_ref {
            require_installed_schedule_owner_in_state(&state, owner, author_actor_ref)?;
        }
        let Some(record) = state.admission.admission(integration_ref, request_id) else {
            return Ok(None);
        };
        if record
            .source_input_fingerprint
            .as_deref()
            .is_some_and(|fingerprint| {
                fingerprint.starts_with(OWNER_NAMED_ACTION_FINGERPRINT_PREFIX)
            })
        {
            return invalid("installed action recovery requires its original owner destination");
        }
        let retained_conversation = state
            .admission
            .full_conversation(&record.conversation_ref)?
            .ok_or_else(|| invalid_error("request ID does not identify a conversation"))?;
        // Schedule action receipts admitted through an owner destination must
        // be recovered through a selected owner or retained conversation. The
        // current default context can change after admission, so it cannot
        // identify the original destination mode.
        if conversation_ref.is_none()
            && owner_context_ref.is_none()
            && matches!(
                activation,
                "@cwr--list--schedules"
                    | "@cwr--get--schedule"
                    | "@cwr--pause--schedule"
                    | "@cwr--resume--schedule"
                    | "@cwr--list--schedule_runs"
            )
        {
            return invalid("schedule recovery requires its installed owner destination");
        }
        let invocation = record
            .invocation_ref
            .as_deref()
            .and_then(|id| state.admission.invocation_record(id))
            .ok_or_else(|| invalid_error("request ID does not identify an action admission"))?;
        let text = if input.as_object().is_some_and(Map::is_empty) {
            activation.to_owned()
        } else {
            format!("{activation} {}", serde_json::to_string(input)?)
        };
        let ingress = ConversationIngress {
            owner_context_ref: if conversation_ref.is_none() { retained_conversation.owner_context_ref.clone() } else { None },
            file_refs: Vec::new(),
            integration_ref: integration_ref.into(),
            provider_event_ref: request_id.into(),
            author_actor_ref: author_actor_ref.into(),
            conversation: conversation_ref.map_or(
                ConversationIngressConversation::CreateForAction,
                |id| ConversationIngressConversation::Continue {
                    conversation_ref: id.into(),
                },
            ),
            addressing: ConversationIngressAddressing::Action {
                actor_ref: invocation.target_actor_ref.clone(),
                activation: activation.into(),
                action_name: invocation.action_name.clone(),
                input: input.clone(),
            },
            text,
        };
        let record = state
            .admission
            .recover_action(&ingress)?
            .ok_or_else(|| invalid_error("retained action admission disappeared"))?;
        let conversation = state
            .admission
            .full_conversation(&record.conversation_ref)?
            .filter(|c| {
                owner_context_ref.is_none_or(|owner| c.owner_context_ref.as_deref() == Some(owner))
                    && c.participant_actor_refs
                    .iter()
                    .any(|a| a == author_actor_ref)
            })
            .ok_or_else(|| invalid_error("action recovery is unavailable to this actor"))?;
        Ok(Some(LocalAdmission {
            record,
            conversation,
        }))
    }

    pub fn conversation(&self, conversation_ref: &str) -> CwrResult<Option<RetainedConversation>> {
        self.lock()?.admission.full_conversation(conversation_ref)
    }

    pub fn conversations(&self) -> CwrResult<Vec<RetainedConversation>> {
        let mut state = self.lock()?;
        state.admission.hydrate_all_cold_conversations()?;
        Ok(state.admission.conversations().cloned().collect())
    }

    pub fn host_conversation(
        &self,
        conversation_ref: &str,
    ) -> CwrResult<Option<RetainedConversation>> {
        let mut state = self.lock()?;
        state.admission.hydrate_all_cold_conversations()?;
        Ok(state
            .admission
            .conversation(conversation_ref)
            .map(|conversation| {
                crate::v2026_09::host::project_conversation_with_authoritative_work(
                    conversation,
                    &state.admission,
                )
            }))
    }

    pub fn host_conversations(&self) -> CwrResult<Vec<RetainedConversation>> {
        let mut state = self.lock()?;
        state.admission.hydrate_all_cold_conversations()?;
        Ok(state
            .admission
            .conversations()
            .map(|conversation| {
                crate::v2026_09::host::project_conversation_with_authoritative_work(
                    conversation,
                    &state.admission,
                )
            })
            .collect())
    }

    pub fn host_conversation_metadata(
        &self,
        conversation_ref: &str,
    ) -> CwrResult<Option<RetainedConversation>> {
        let state = self.lock()?;
        crate::v2026_09::host::project_host_conversation_metadata(
            &state.admission,
            conversation_ref,
        )
    }

    pub fn host_conversation_bootstrap(
        &self,
        include_lineage: bool,
    ) -> CwrResult<Vec<RetainedConversation>> {
        let state = self.lock()?;
        crate::v2026_09::host::project_host_conversation_bootstrap(
            &state.admission,
            include_lineage,
        )
    }

    pub fn conversation_metadata(conversation: &RetainedConversation) -> RetainedConversation {
        crate::v2026_09::host::project_full_conversation_metadata(conversation)
    }

    pub fn conversation_ref_for_invocation(
        &self,
        invocation_ref: &str,
    ) -> CwrResult<Option<String>> {
        Ok(self
            .lock()?
            .admission
            .invocation_record(invocation_ref)
            .and_then(|record| record.conversation_ref.clone()))
    }

    pub fn published_progress_conversation_ref(
        &self,
        source_activity_ref: &str,
    ) -> CwrResult<Option<String>> {
        self
            .lock()?
            .admission
            .published_progress_conversation_ref(source_activity_ref)
    }

    pub fn terminal_execution_for_invocation(
        &self,
        invocation_ref: &str,
    ) -> CwrResult<Option<(String, String, ActionResponse)>> {
        let state = self.lock()?;
        Ok(state
            .admission
            .invocation_record(invocation_ref)
            .filter(|record| record.state != ActionInvocationState::Stopping)
            .and_then(|record| {
                Some((
                    record.conversation_ref.clone()?,
                    record.target_actor_ref.clone(),
                    record.terminal_response()?.clone(),
                ))
            }))
    }

    pub fn conversation_turn_statuses(
        &self,
    ) -> CwrResult<BTreeMap<String, Vec<crate::v2026_09::host::ConversationTurnStatus>>> {
        Ok(crate::v2026_09::host::project_conversation_turn_statuses(
            &self.lock()?.admission,
        ))
    }

    /// Select and project affected turn-status owners against one authoritative state.
    pub fn conversation_turn_statuses_affected_by(
        &self,
        invocation_refs: &BTreeSet<String>,
        conversation_refs: &BTreeSet<String>,
    ) -> CwrResult<BTreeMap<String, Vec<crate::v2026_09::host::ConversationTurnStatus>>> {
        let state = self.lock()?;
        let selected = crate::v2026_09::host::affected_turn_status_conversations(
            &state.admission,
            invocation_refs,
            conversation_refs,
        );
        Ok(crate::v2026_09::host::project_conversation_turn_statuses_for(
            &state.admission,
            &selected,
        ))
    }

    pub fn explicit_reply_recipient_actor_refs(&self, message_ref: &str) -> CwrResult<Vec<String>> {
        let state = self.lock()?;
        Ok(state
            .admission
            .admissions()
            .filter(|record| {
                record.reply_message_ref.as_deref() == Some(message_ref)
                    && record.reply_recipient_visibility
                        == crate::v2026_09::conversation::ConversationRecipientVisibility::Explicit
            })
            .filter_map(|record| record.invocation_ref.as_deref())
            .filter_map(|invocation_ref| state.admission.invocation_record(invocation_ref))
            .map(|invocation| invocation.requester_actor_ref.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    pub fn reply_invocation_ref(&self, message_ref: &str) -> CwrResult<Option<String>> {
        Ok(self
            .lock()?
            .admission
            .admissions()
            .find(|record| record.reply_message_ref.as_deref() == Some(message_ref))
            .and_then(|record| record.invocation_ref.clone()))
    }

    pub fn delivery_invocation_ref(
        &self,
        message: &crate::v2026_09::conversation::ConversationMessage,
    ) -> CwrResult<Option<String>> {
        let state = self.lock()?;
        let direct = state
            .admission
            .admissions()
            .find(|record| record.reply_message_ref.as_deref() == Some(&message.message_ref))
            .and_then(|record| record.invocation_ref.clone());
        if direct.is_some() {
            return Ok(direct);
        }
        let progress_source_message_ref = message
            .progress
            .as_ref()
            .and(message.reply_to_message_ref.as_deref());
        Ok(progress_source_message_ref.and_then(|source_message_ref| {
            state
                .admission
                .admissions()
                .find(|record| record.source_message_ref == source_message_ref)
                .and_then(|record| record.invocation_ref.clone())
        }))
    }

    pub fn inspect_invocation(
        &self,
        selector: &str,
    ) -> CwrResult<crate::v2026_09::conversation::InvocationDebugReport> {
        let state = self.lock()?;
        let mut report = state.admission.inspect_invocation(selector)?;
        if state.graph.permissions.allows_all() {
            state.admission.include_original_invocation(&mut report)?;
        }
        drop(state);
        report.diagnostics_directory = Some(self.diagnostics.directory.display().to_string());
        report.diagnostic_evidence = Some(Box::new(
            self.diagnostics
                .evidence_for_invocation(&report.invocation_ref),
        ));
        Ok(report)
    }

    pub fn inspect_latest_invocation_for_conversation(
        &self,
        conversation_ref: &str,
    ) -> CwrResult<crate::v2026_09::conversation::InvocationDebugReport> {
        let state = self.lock()?;
        let mut report = state.admission.inspect_latest_invocation_for_conversation(conversation_ref)?;
        if state.graph.permissions.allows_all() {
            state.admission.include_original_invocation(&mut report)?;
        }
        drop(state);
        report.diagnostics_directory = Some(self.diagnostics.directory.display().to_string());
        report.diagnostic_evidence = Some(Box::new(
            self.diagnostics
                .evidence_for_invocation(&report.invocation_ref),
        ));
        Ok(report)
    }
}

impl LocalRuntimeState {
    fn settle_stopped_invocation(&mut self, invocation: &str, now: u64) -> CwrResult<()> {
        let settled = self.admission.settle_stopped_invocation(invocation, now)?;
        self.settled_stop_invocations.extend(settled);
        Ok(())
    }
}
impl LocalRuntime {
    /// Every transition, including ancestors settled by a child, is available for publication.
    pub fn take_settled_stop_invocations(&self) -> CwrResult<Vec<String>> {
        Ok(std::mem::take(&mut self.lock()?.settled_stop_invocations))
    }
}
