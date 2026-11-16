impl ConversationAdmissionStore {
    pub(crate) fn read_store_identity(&self) -> (PathBuf, String) {
        (self.state_path.clone(), self.runtime_root_ref().to_owned())
    }

    pub(crate) fn read_store_location(&self) -> ReadStoreLocation {
        ReadStoreLocation::new(self.backend.clone(), self.state_path.clone(), self.runtime_root_ref().to_owned())
    }
    pub fn read_store(&self) -> CwrResult<ConversationReadStore> {
        self.read_store_location().open_until(std::time::Instant::now() + std::time::Duration::from_secs(2), &ConversationReadCancellation::pending())
    }

    pub fn conversation(&self, conversation_ref: &str) -> Option<&RetainedConversation> {
        assert!(
            !self.state.cold_conversation_refs.contains(conversation_ref),
            "cold conversation history must be loaded before borrowing it"
        );
        self.conversation_header(conversation_ref)
    }

    /// Borrow retained metadata. A cold conversation's messages and activity
    /// are empty until its history is loaded.
    pub(crate) fn conversation_header(&self, conversation_ref: &str) -> Option<&RetainedConversation> {
        self.state
            .conversations
            .iter()
            .find(|conversation| conversation.conversation_ref == conversation_ref)
    }

    pub(crate) fn full_conversation(
        &self,
        conversation_ref: &str,
    ) -> CwrResult<Option<RetainedConversation>> {
        Ok(self.conversation_history(conversation_ref)?.map(std::borrow::Cow::into_owned))
    }

    pub(crate) fn conversation_history(
        &self,
        conversation_ref: &str,
    ) -> CwrResult<Option<std::borrow::Cow<'_, RetainedConversation>>> {
        if !self.state.cold_conversation_refs.contains(conversation_ref) {
            return Ok(self.conversation(conversation_ref).map(std::borrow::Cow::Borrowed));
        }
        let conversation = if self.conversation_header(conversation_ref).is_some() {
            Some(self.backend.lock().map_err(|_| invalid_error("conversation backend lock poisoned"))?.conversation(conversation_ref)?)
        } else { None };
        if let Some(conversation) = &conversation {
            for message in &conversation.messages {
                visualization::validate_persisted(message)?;
            }
        }
        Ok(conversation.map(std::borrow::Cow::Owned))
    }

    pub(crate) fn bootstrap_conversation_metadata_sources(
        &self,
    ) -> CwrResult<Vec<RetainedConversation>> {
        let mut summaries = if self.state.cold_conversation_refs.is_empty() {
            BTreeMap::new()
        } else {
            self.backend.lock().map_err(|_| invalid_error("conversation backend lock poisoned"))?.metadata_sources(None)?
        };
        let mut sources = Vec::new();
        for conversation in self.state.conversations.iter() {
            if conversation.parent_conversation_ref.is_some() {
                continue;
            }
            let mut source = conversation.clone();
            if self.state.cold_conversation_refs.contains(&source.conversation_ref) {
                if let Some((first_message, lineage)) = summaries.remove(&source.conversation_ref) {
                    source.messages = first_message.into_iter().collect();
                    source.activity = lineage;
                }
            }
            sources.push(source);
        }
        Ok(sources)
    }

    pub(crate) fn conversation_metadata_source(
        &self,
        conversation_ref: &str,
    ) -> CwrResult<Option<RetainedConversation>> {
        let Some(conversation) = self.conversation_header(conversation_ref) else {
            return Ok(None);
        };
        let mut source = conversation.clone();
        if self.state.cold_conversation_refs.contains(conversation_ref) {
            let (first_message,lineage)=self.backend.lock().map_err(|_| invalid_error("conversation backend lock poisoned"))?.metadata_sources(Some(conversation_ref))?.remove(conversation_ref).unwrap_or_default();
            source.messages = first_message.into_iter().collect();
            source.activity = lineage;
        }
        Ok(Some(source))
    }

    pub(super) fn hydrate_all_cold_conversations(&mut self) -> CwrResult<()> {
        if self.state.cold_conversation_refs.is_empty() {
            return Ok(());
        }
        let mut conversations=Vec::new();
        {
            let mut backend=self.backend.lock().map_err(|_| invalid_error("conversation backend lock poisoned"))?;
            backend.set_expected_revision(self.commit_revision);
            for conversation in self.state.conversations.iter() {conversations.push(backend.conversation(&conversation.conversation_ref)?);}
        }
        let previous = std::mem::replace(&mut self.state.conversations, conversations.into());
        let validation = (|| -> CwrResult<()> {
            invocation::validate_invocation_execution_links(&self.state)?;
            timeline::validate_retained_timelines(&self.state)?;
            visualization::validate_artifacts(&self.state)?;
            let mut backend=self.backend.lock().map_err(|_| invalid_error("conversation backend lock poisoned"))?;
            for reference in self.state.conversations.iter().flat_map(|conversation|&conversation.messages).flat_map(|message|&message.visualizations).filter_map(|visualization|visualization.artifact_ref.as_deref()) {
                if backend.get(reference)?.is_none() {return invalid("Conversation visualization artifact is unavailable");}
            }
            Ok(())
        })();
        if let Err(error) = validation {
            self.state.conversations = previous;
            return Err(error);
        }
        self.state.cold_conversation_refs.clear();
        Ok(())
    }

    pub fn conversations(&self) -> impl Iterator<Item = &RetainedConversation> {
        assert!(
            self.state.cold_conversation_refs.is_empty(),
            "cold conversation history must be loaded before listing full conversations"
        );
        self.state.conversations.iter()
    }

    /// Histories needed by unresolved work are loaded at open. A status read
    /// may project those while unrelated, completed histories remain cold.
    pub(crate) fn hydrated_conversations(&self) -> impl Iterator<Item = &RetainedConversation> {
        self.state.conversations.iter().filter(|conversation| {
            !self.state.cold_conversation_refs.contains(&conversation.conversation_ref)
        })
    }

    pub(crate) fn hydrated_execution_conversation(
        &self,
        invocation_ref: &str,
    ) -> Option<&RetainedConversation> {
        let reference = self.execution_conversation_ref(invocation_ref)?;
        (!self.state.cold_conversation_refs.contains(reference))
            .then(|| self.conversation_header(reference))
            .flatten()
    }

    pub fn admissions(&self) -> impl Iterator<Item = &ConversationAdmissionRecord> {
        self.state.admissions.iter()
    }

    pub(crate) fn admissions_for_conversation<'a>(
        &'a self,
        conversation_ref: &'a str,
    ) -> impl DoubleEndedIterator<Item = &'a ConversationAdmissionRecord> + 'a {
        self.admission_indices_by_conversation
            .get(conversation_ref)
            .into_iter()
            .flatten()
            .map(|index| &self.state.admissions[*index])
    }

    pub fn runtime_root_ref(&self) -> &str {
        &self.state.runtime_root_ref
    }

    fn last_addressed_route(
        &self,
        conversation: &RetainedConversation,
        author_actor_ref: &str,
    ) -> Option<(String, Option<String>)> {
        let imported = crate::v2026_09::conversation::transfer::is_imported_conversation_ref(
            &conversation.conversation_ref,
        );
        conversation.messages.iter().rev().find_map(|message| {
            if message.author_actor_ref != author_actor_ref
                || message.addressed_actor_refs.len() != 1
            {
                return None;
            }
            let target_actor_ref = message.addressed_actor_refs.first()?.clone();
            let admission = self.state.admissions.iter().rev().find(|record| {
                record.conversation_ref == conversation.conversation_ref
                    && record.source_message_ref == message.message_ref
            });
            match admission {
                Some(admission) => {
                    if admission.target_actor_ref.as_ref()? != &target_actor_ref {
                        return None;
                    }
                    Some((target_actor_ref, admission.actor_integration_ref.clone()))
                }
                // Imported conversations intentionally do not retain admission records.
                // The graph validation performed by the caller makes a historical
                // addressed actor a safe route for the first continuation.
                None if imported => Some((target_actor_ref, None)),
                None => None,
            }
        })
    }

    pub fn execution_conversation(&self, invocation_ref: &str) -> Option<&RetainedConversation> {
        let conversation_ref = self.execution_conversation_ref(invocation_ref)?;
        self.conversation(conversation_ref)
    }

    pub fn append_execution_activity(
        &mut self,
        invocation_ref: &str,
        event: ConversationActivityEvent,
    ) -> CwrResult<ConversationActivity> {
        self.transact(|state| {
            invocation::push_execution_activity_in_state(
                state,
                invocation_ref,
                event,
                current_unix_ms(),
            )
        })
    }

    pub fn retain_execution_invocation(
        &mut self,
        invocation_ref: &str,
        invocation: &AdapterInvocation,
    ) -> CwrResult<()> {
        let conversation_ref = self
            .execution_conversation_ref(invocation_ref)
            .map(str::to_owned)
            .ok_or_else(|| invalid_error("invocation has no execution conversation"))?;
        let execution = self.full_conversation(&conversation_ref)?;
        let started = execution
            .as_ref()
            .and_then(|conversation| {
                conversation
                    .activity
                    .iter()
                    .find_map(|activity| match &activity.event {
                        ConversationActivityEvent::TurnStarted { invocation, .. } => {
                            invocation.as_ref()
                        }
                        _ => None,
                    })
            })
            .ok_or_else(|| invalid_error("execution conversation has no invocation"))?;
        let retained: AdapterInvocation = serde_json::from_value(started.clone())?;
        if retained.invocation_ref != invocation_ref
            || invocation.invocation_ref != invocation_ref
            || retained.action_name != invocation.action_name
            || retained.input != invocation.input
            || retained.output_schema != invocation.output_schema
            || retained.route_fingerprint != invocation.route_fingerprint
        {
            return invalid("prepared execution changed its retained request or result contract");
        }
        let source = context::reference(&invocation.thread)?;
        let original_source = context::reference(&retained.thread)?;
        let preserve_legacy_source = context::has_embedded_messages(&retained.thread)
            && context::restore(&self.state, &original_source).is_err();
        let initializes_own_context = retained.model_run_ref.is_none()
            && invocation.execution_scope
                == crate::action_contract::ExecutionScope::OwnConversation
            && source["conversation_ref"].as_str() == Some(conversation_ref.as_str())
            && source.get("requesting_context") == Some(&original_source);
        let mut without_directory = source.clone();
        if let Some(object) = without_directory.as_object_mut() {
            object.remove("coworkers");
        }
        let initializes_directory = original_source.get("coworkers").is_none()
            && retained.model_run_ref.is_none()
            && without_directory == original_source;
        if source != original_source && !initializes_own_context && !initializes_directory {
            return invalid("prepared execution changed its recorded conversation context");
        }
        let snapshot = if invocation.continuation.is_some() {
            let Some(integration_ref) = invocation.integration_ref.as_deref() else {
                return Ok(());
            };
            let mut retained = retained;
            let mut changed = false;
            match retained.integration_ref.as_deref() {
                Some(retained_ref) if retained_ref != integration_ref => {
                    return invalid(
                        "retained adapter invocation has a different provider integration ref",
                    )
                }
                Some(_) => {}
                None => {
                    retained.integration_ref = Some(integration_ref.into());
                    changed = true;
                }
            }
            if retained.retained_route.is_none() && invocation.retained_route.is_some() {
                retained.retained_route = invocation.retained_route.clone();
                changed = true;
            }
            if !changed {
                return Ok(());
            }
            serde_json::to_value(retained)?
        } else {
            let mut snapshot = context::invocation_snapshot(invocation)?;
            if preserve_legacy_source {
                snapshot["thread"] = retained.thread;
            }
            snapshot
        };
        self.transact(|state| {
            let started = state
                .conversations
                .find_mut(|conversation| conversation.conversation_ref == conversation_ref)
                .and_then(|conversation| {
                    conversation.activity.iter_mut().find_map(|activity| {
                        match &mut activity.event {
                            ConversationActivityEvent::TurnStarted { invocation, .. } => {
                                Some(invocation)
                            }
                            _ => None,
                        }
                    })
                })
                .ok_or_else(|| invalid_error("execution conversation has no turn start"))?;
            *started = Some(snapshot);
            Ok(())
        })
    }

    pub fn is_admitted(&self, record: &ConversationAdmissionRecord) -> bool {
        self.find_admission(&record.integration_ref, &record.provider_event_ref)
            .is_some_and(|current| {
                current.state == ConversationAdmissionState::Admitted
                    && current.invocation_ref == record.invocation_ref
            })
    }

    pub fn resume_admitted(
        &self,
        record: &ConversationAdmissionRecord,
    ) -> CwrResult<Option<AdmittedConversationTurn>> {
        let Some(record) = self.find_admission(&record.integration_ref, &record.provider_event_ref)
        else {
            return Ok(None);
        };
        if record.state != ConversationAdmissionState::Admitted {
            return Ok(None);
        }
        let Some(invocation_ref) = record.invocation_ref.as_deref() else {
            return Ok(None);
        };
        // Older state could retain a terminal ledger record while leaving its
        // admission marked admitted. Never turn that projection mismatch into
        // a second execution after an upgrade.
        if self
            .state
            .invocation_ledger
            .invocation(invocation_ref)
            .is_some_and(|invocation| invocation.state.is_terminal())
        {
            return Ok(None);
        }
        let Some(execution_conversation) = self.execution_conversation(invocation_ref).cloned()
        else {
            return Ok(None);
        };
        let invocation = execution_conversation.activity.iter().find_map(|activity| {
            let ConversationActivityEvent::TurnStarted { invocation, .. } = &activity.event else {
                return None;
            };
            invocation.clone()
        });
        let Some(invocation) = invocation else {
            return Ok(None);
        };
        let mut invocation: AdapterInvocation = serde_json::from_value(invocation)?;
        let conversation = self
            .conversation(&record.conversation_ref)
            .cloned()
            .ok_or_else(|| invalid_error("admitted turn conversation is unavailable"))?;
        invocation.thread = context::restore(&self.state, &invocation.thread)?;
        if let Some(checkpoint) = context::checkpoint(&execution_conversation, &invocation)? {
            invocation.continuation = Some(checkpoint);
        }
        Ok(Some(AdmittedConversationTurn {
            record: record.clone(),
            conversation,
            execution_conversation: Some(execution_conversation),
            invocation: Some(invocation),
        }))
    }
}
