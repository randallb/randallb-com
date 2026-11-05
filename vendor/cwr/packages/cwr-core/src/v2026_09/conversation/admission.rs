impl ConversationAdmissionStore {
    pub fn prepare(
        &self,
        graph: &ResolvedCwrGraph,
        ingress: &ConversationIngress,
        actor_integration_ref: Option<&str>,
    ) -> CwrResult<ConversationPreparationOutcome> {
        self.prepare_with_preferred_binding(graph, ingress, actor_integration_ref, None)
    }

    fn prepare_with_preferred_binding(
        &self,
        graph: &ResolvedCwrGraph,
        ingress: &ConversationIngress,
        actor_integration_ref: Option<&str>,
        preferred_binding: Option<&str>,
    ) -> CwrResult<ConversationPreparationOutcome> {
        validate_ingress(graph, ingress)?;
        if ingress.file_refs.len() > 4 {
            return invalid("Messages may contain at most four files");
        }
        for reference in &ingress.file_refs {
            self.file_metadata(&ingress.author_actor_ref, reference)?;
        }
        let ingress_fingerprint = conversation_ingress_fingerprint(ingress)?;
        if let Some(record) = self.replayed_admission(ingress, &ingress_fingerprint)? {
            return Ok(ConversationPreparationOutcome::Existing(record));
        }
        if matches!(
            ingress.addressing,
            ConversationIngressAddressing::Unaddressed
        ) && actor_integration_ref.is_some()
        {
            return invalid(
                "an actor integration ref cannot be supplied for an unaddressed message",
            );
        }

        let mut conversation = self.ingress_conversation(ingress)?;
        validate_owner_context(ingress, &conversation)?;
        let (target_actor_ref, requested_actor_integration_ref) = self.message_recipient(
            graph,
            ingress,
            &conversation,
            actor_integration_ref,
            preferred_binding,
        )?;
        self.prepare_participants(
            graph,
            &mut conversation,
            ingress,
            target_actor_ref.as_deref(),
        )?;

        let source_message_ref = self.new_ref("messages");
        conversation.record_message(ConversationMessage {
            file_refs: ingress.file_refs.clone(),
            progress: None,
            message_ref: source_message_ref.clone(),
            author_actor_ref: ingress.author_actor_ref.clone(),
            addressed_actor_refs: target_actor_ref
                .iter()
                .filter(|target| {
                    !graph
                        .gateways
                        .iter()
                        .any(|gateway| gateway.resource_ref == **target)
                })
                .cloned()
                .collect(),
            addressing_mode: Some(message_addressing_mode(&ingress.addressing)),
            text: ingress.text.clone(),
            visualizations: Vec::new(),
            reply_to_message_ref: None,
            action: match &ingress.addressing {
                ConversationIngressAddressing::Action {
                    activation, input, ..
                } => Some(ConversationActionEvent::Call {
                    activation: activation.clone(),
                    input: input.clone(),
                }),
                _ => None,
            },
        });

        let (selected_actor_integration_ref, selected_adapter_ref, invocation_ref, invocation) =
            if let Some(target_actor_ref) = &target_actor_ref {
                let requested_integration = requested_actor_integration_ref.as_deref();
                match &ingress.addressing {
                    ConversationIngressAddressing::Action {
                        action_name, input, ..
                    } => {
                        let actor_integration_ref = required_action_integration_ref(
                            target_actor_ref,
                            requested_integration,
                        )?;
                        let invocation_ref = self.new_ref("invocations");
                        let selected = select_action_route(
                            graph,
                            target_actor_ref,
                            actor_integration_ref,
                            action_name,
                        )?;
                        let mut invocation = selected.project_adapter_invocation(
                            invocation_ref.clone(),
                            input.clone(),
                            conversation_thread(&conversation),
                        )?;
                        invocation.source_actor_ref = Some(ingress.author_actor_ref.clone());
                        (
                            Some(selected.actor_integration_ref.clone()),
                            Some(selected.adapter_ref.clone()),
                            Some(invocation_ref),
                            Some(invocation),
                        )
                    }
                    _ if requested_integration.is_some() => {
                        let actor_integration_ref =
                            requested_integration.expect("presence checked");
                        let invocation_ref = self.new_ref("invocations");
                        let selected = select_actor_integration(
                            graph,
                            target_actor_ref,
                            actor_integration_ref,
                        )?;
                        let mut invocation = selected.project_adapter_invocation(
                            invocation_ref.clone(),
                            RESPOND_TO_MESSAGE_ACTION_NAME,
                            json!({"source_message_ref": source_message_ref}),
                            respond_to_message_output_schema(),
                            conversation_thread(&conversation),
                        );
                        invocation.source_actor_ref = Some(ingress.author_actor_ref.clone());
                        (
                            Some(selected.actor_integration_ref.clone()),
                            Some(selected.adapter_ref.clone()),
                            Some(invocation_ref),
                            Some(invocation),
                        )
                    }
                    _ => (None, None, None, None),
                }
            } else {
                if requested_actor_integration_ref.is_some() {
                    return invalid(
                        "an actor integration ref cannot be selected for an unaddressed message",
                    );
                }
                (None, None, None, None)
            };

        let invocation_snapshot = invocation
            .as_ref()
            .map(context::invocation_snapshot)
            .transpose()?;
        let execution_conversation = invocation_ref.as_ref().map(|invocation_ref| {
            let conversation_ref = self.new_ref("conversations");
            RetainedConversation {
                owner_context_ref: conversation.owner_context_ref.clone(),
                conversation_ref,
                title: None,
                archived: false,
                sidebar_eligible: None,
                parent_conversation_ref: Some(conversation.conversation_ref.clone()),
                participant_actor_refs: target_actor_ref.iter().cloned().collect(),
                messages: Vec::new(),
                timeline_connection_ref: None,
                activity: vec![ConversationActivity {
                    activity_ref: self.new_ref("activities"),
                    sequence: 1,
                    occurred_at_unix_ms: current_unix_ms(),
                    event: ConversationActivityEvent::TurnStarted {
                        invocation_ref: invocation_ref.clone(),
                        source_message_ref: source_message_ref.clone(),
                        invocation: invocation_snapshot.clone(),
                    },
                }],
            }
        });
        let record = ConversationAdmissionRecord {
            integration_ref: ingress.integration_ref.clone(),
            provider_event_ref: ingress.provider_event_ref.clone(),
            ingress_fingerprint: Some(ingress_fingerprint),
            source_input_fingerprint: None,
            conversation_ref: conversation.conversation_ref.clone(),
            source_message_ref,
            execution_conversation_ref: execution_conversation
                .as_ref()
                .map(|conversation| conversation.conversation_ref.clone()),
            target_actor_ref,
            actor_integration_ref: selected_actor_integration_ref,
            adapter_ref: selected_adapter_ref,
            invocation_ref,
            state: if invocation.is_some() {
                ConversationAdmissionState::Admitted
            } else {
                ConversationAdmissionState::Recorded
            },
            reply_recipient_visibility: ConversationRecipientVisibility::Implicit,
            reply_message_ref: None,
        };
        Ok(ConversationPreparationOutcome::New(Box::new(
            PreparedConversationAdmission {
                record,
                conversation,
                execution_conversation,
                invocation,
            },
        )))
    }

    /// Identify the new message recipient without running provider processes.
    /// LocalRuntime uses this on both sides of its unlocked readiness probe.
    pub(crate) fn message_preference_actor(
        &self,
        graph: &ResolvedCwrGraph,
        ingress: &ConversationIngress,
        exact_binding: Option<&str>,
    ) -> CwrResult<Option<String>> {
        validate_ingress(graph, ingress)?;
        validate_owner_context(ingress, &self.ingress_conversation(ingress)?)?;
        if self
            .replayed_admission(ingress, &conversation_ingress_fingerprint(ingress)?)?
            .is_some()
            || exact_binding.is_some()
            || matches!(
                ingress.addressing,
                ConversationIngressAddressing::Action { .. }
            )
        {
            return Ok(None);
        }
        let mut target = addressed_actor_ref(graph, &ingress.addressing)?;
        if target.is_none()
            && matches!(
                ingress.addressing,
                ConversationIngressAddressing::Unaddressed
            )
        {
            if let ConversationIngressConversation::Continue { conversation_ref } =
                &ingress.conversation
            {
                let conversation = self.full_conversation(conversation_ref)?.ok_or_else(|| {
                    invalid_error(format!("unknown conversation ref {conversation_ref}"))
                })?;
                target = self
                    .last_addressed_route(&conversation, &ingress.author_actor_ref)
                    .map(|route| route.0);
            }
        }
        Ok(target.filter(|reference| {
            graph.actors.iter().any(|actor| {
                &actor.actor_ref == reference && !actor.message_binding_preference.is_empty()
            })
        }))
    }

    fn message_recipient(
        &self,
        graph: &ResolvedCwrGraph,
        ingress: &ConversationIngress,
        conversation: &RetainedConversation,
        actor_integration_ref: Option<&str>,
        preferred_binding: Option<&str>,
    ) -> CwrResult<(Option<String>, Option<String>)> {
        let mut target_actor_ref = addressed_actor_ref(graph, &ingress.addressing)?;
        let mut requested_actor_integration_ref = actor_integration_ref.map(str::to_owned);
        if target_actor_ref.is_none()
            && matches!(
                ingress.addressing,
                ConversationIngressAddressing::Unaddressed
            )
        {
            if let Some((inherited_actor_ref, inherited_integration_ref)) =
                self.last_addressed_route(conversation, &ingress.author_actor_ref)
            {
                validate_addressed_actor_ref(graph, &inherited_actor_ref)?;
                let has_preference = graph.actors.iter().any(|actor| {
                    actor.actor_ref == inherited_actor_ref
                        && !actor.message_binding_preference.is_empty()
                });
                target_actor_ref = Some(inherited_actor_ref);
                requested_actor_integration_ref = if has_preference {
                    None
                } else {
                    inherited_integration_ref.or_else(|| {
                        graph
                            .actors
                            .iter()
                            .find(|actor| Some(&actor.actor_ref) == target_actor_ref.as_ref())
                            .and_then(|actor| actor.default_actor_integration_ref.clone())
                    })
                };
            }
        }
        if requested_actor_integration_ref.is_none()
            && !matches!(
                ingress.addressing,
                ConversationIngressAddressing::Action { .. }
            )
        {
            if let Some(actor_ref) = &target_actor_ref {
                requested_actor_integration_ref = match preferred_binding {
                    Some(binding) => Some(binding.to_owned()),
                    None => graph.preferred_message_binding(actor_ref)?,
                };
            }
        }
        if actor_integration_ref.is_none()
            && matches!(
                ingress.addressing,
                ConversationIngressAddressing::Unaddressed
            )
            && requested_actor_integration_ref
                .as_ref()
                .is_some_and(|reference| {
                    !graph
                        .actor_integrations
                        .iter()
                        .any(|binding| &binding.actor_integration_ref == reference)
                        && self.admissions().any(|record| {
                            record.actor_integration_ref.as_ref() == Some(reference)
                                && record.adapter_ref.as_deref()
                                    == Some(crate::adapter::PERSON_ADAPTER_REF)
                        })
                })
        {
            requested_actor_integration_ref = None;
        }
        Ok((target_actor_ref, requested_actor_integration_ref))
    }

    pub fn commit(
        &mut self,
        prepared: ConversationPreparationOutcome,
    ) -> CwrResult<ConversationAdmissionOutcome> {
        let ConversationPreparationOutcome::New(mut prepared) = prepared else {
            let ConversationPreparationOutcome::Existing(record) = prepared else {
                unreachable!("conversation preparation variants are exhaustive")
            };
            return Ok(ConversationAdmissionOutcome::Existing(record));
        };
        self.hydrate_all_cold_conversations()?;
        if let Some(record) = self.find_admission(
            &prepared.record.integration_ref,
            &prepared.record.provider_event_ref,
        ) {
            if let (Some(prepared_fingerprint), Some(existing_fingerprint)) = (
                prepared.record.ingress_fingerprint.as_deref(),
                record.ingress_fingerprint.as_deref(),
            ) {
                if prepared_fingerprint != existing_fingerprint {
                    return invalid(format!(
                        "conversation ingress event {} was reused with different input",
                        prepared.record.provider_event_ref
                    ));
                }
            }
            return Ok(ConversationAdmissionOutcome::Existing(Box::new(
                record.clone(),
            )));
        }

        if prepared.conversation.owner_context_ref.as_deref().is_some_and(|owner| owner.trim().is_empty()) {
            return invalid("conversation owner context must not be blank");
        }
        if prepared.execution_conversation.as_ref().is_some_and(|execution| execution.owner_context_ref != prepared.conversation.owner_context_ref) {
            return invalid("execution conversation must retain its requesting conversation owner");
        }
        if let Some(current) = self.conversation(&prepared.record.conversation_ref) {
            if prepared.conversation.owner_context_ref != current.owner_context_ref {
                return invalid("an existing conversation cannot change owner context");
            }
            if !prepared
                .conversation
                .messages
                .starts_with(&current.messages)
                || !prepared
                    .conversation
                    .activity
                    .starts_with(&current.activity)
            {
                return invalid("Conversation changed during admission; retry this message.");
            }
            prepared.conversation.title = current.title.clone();
            prepared.conversation.archived = current.archived;
        }

        if let Some(invocation) = &mut prepared.invocation {
            if invocation
                .thread
                .as_object()
                .is_some_and(|thread| thread.is_empty())
            {
                invocation.thread = conversation_thread(&prepared.conversation);
            }
            if let Some(execution) = &mut prepared.execution_conversation {
                for activity in &mut execution.activity {
                    if let ConversationActivityEvent::TurnStarted {
                        invocation: snapshot,
                        ..
                    } = &mut activity.event
                    {
                        *snapshot = Some(context::invocation_snapshot(invocation)?);
                    }
                }
            }
        }
        let mut outcome = self.transact(|staged| {
            invocation::stage_root_invocation(staged, &prepared)?;
            if let Some(existing) = staged.conversations.find_mut(|conversation| {
                conversation.conversation_ref == prepared.conversation.conversation_ref
            }) {
                *existing = prepared.conversation.clone();
            } else {
                staged.conversations.push(prepared.conversation.clone());
            }
            if let Some(execution_conversation) = &prepared.execution_conversation {
                staged.conversations.push(execution_conversation.clone());
            }
            let mut committed_record = prepared.record.clone();
            let joins_unresolved_turn = committed_record.state
                == ConversationAdmissionState::Admitted
                && staged.admissions.iter().any(|record| {
                    record.conversation_ref == committed_record.conversation_ref
                        && record.state == ConversationAdmissionState::Admitted
                });
            if joins_unresolved_turn {
                for record in staged.admissions.matching_mut(|record| {
                    record.conversation_ref == committed_record.conversation_ref
                        && record.state == ConversationAdmissionState::Admitted
                }) {
                    record.reply_recipient_visibility = ConversationRecipientVisibility::Explicit;
                }
                committed_record.reply_recipient_visibility =
                    ConversationRecipientVisibility::Explicit;
            }
            staged.admissions.push(committed_record.clone());

            Ok(ConversationAdmissionOutcome::Started(Box::new(
                AdmittedConversationTurn {
                    record: committed_record,
                    conversation: prepared.conversation,
                    execution_conversation: prepared.execution_conversation,
                    invocation: prepared.invocation,
                },
            )))
        })?;
        if let ConversationAdmissionOutcome::Started(turn) = &mut outcome {
            turn.conversation = self
                .conversation(&turn.record.conversation_ref)
                .cloned()
                .ok_or_else(|| invalid_error("committed admission conversation disappeared"))?;
        }
        Ok(outcome)
    }

    pub fn admit(
        &mut self,
        graph: &ResolvedCwrGraph,
        ingress: &ConversationIngress,
        actor_integration_ref: Option<&str>,
    ) -> CwrResult<ConversationAdmissionOutcome> {
        let prepared = self.prepare(graph, ingress, actor_integration_ref)?;
        self.commit(prepared)
    }

    pub(crate) fn admit_host_local(
        &mut self,
        graph: &ResolvedCwrGraph,
        ingress: &ConversationIngress,
        actor_integration_ref: Option<&str>,
        host_local_actor_refs: &[String],
        preferred_binding: Option<&str>,
        source_input_fingerprint: Option<&str>,
    ) -> CwrResult<ConversationAdmissionOutcome> {
        let mut prepared = self.prepare_with_preferred_binding(
            graph,
            ingress,
            actor_integration_ref,
            preferred_binding,
        )?;
        match &mut prepared {
            ConversationPreparationOutcome::New(turn) => {
                turn.record.source_input_fingerprint = source_input_fingerprint.map(str::to_owned);
                if let Some(actor_ref) = turn.record.target_actor_ref.as_ref() {
                    if host_local_actor_refs.contains(actor_ref) {
                        turn.conversation
                            .participant_actor_refs
                            .retain(|participant| participant != actor_ref);
                        if let Some(execution) = &mut turn.execution_conversation {
                            execution
                                .participant_actor_refs
                                .retain(|participant| participant != actor_ref);
                        }
                    }
                }
            }
            ConversationPreparationOutcome::Existing(record) => {
                match (
                    record.source_input_fingerprint.as_deref(),
                    source_input_fingerprint,
                ) {
                    (Some(expected), Some(actual)) if expected != actual => {
                        return invalid(format!(
                            "conversation ingress event {} was reused with different source input",
                            ingress.provider_event_ref
                        ));
                    }
                    (None, Some(_)) => {
                        return invalid(format!(
                            "conversation ingress event {} has no retained source input",
                            ingress.provider_event_ref
                        ));
                    }
                    _ => {}
                }
            }
        }
        self.commit(prepared)
    }
}

fn required_action_integration_ref<'a>(
    target_actor_ref: &str,
    actor_integration_ref: Option<&'a str>,
) -> CwrResult<&'a str> {
    actor_integration_ref.ok_or_else(|| {
        invalid_error(format!(
            "action target {target_actor_ref} requires an exact actor integration ref"
        ))
    })
}

fn conversation_ingress_fingerprint(ingress: &ConversationIngress) -> CwrResult<String> {
    // On continuation ownership is an assertion, not a request to change the thread.
    let mut canonical = ingress.clone();
    if matches!(canonical.conversation, ConversationIngressConversation::Continue { .. }) {
        canonical.owner_context_ref = None;
    }
    let encoded = serde_json::to_vec(&canonical)?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(encoded))))
}

impl ConversationAdmissionStore {
    fn ingress_conversation(&self, ingress: &ConversationIngress) -> CwrResult<RetainedConversation> {
        Ok(match &ingress.conversation {
            ConversationIngressConversation::Create
            | ConversationIngressConversation::CreateForAction => RetainedConversation {
                owner_context_ref: ingress.owner_context_ref.clone(),
                conversation_ref: self.new_ref("conversations"),
                title: None,
                archived: false,
                sidebar_eligible: Some(matches!(
                    ingress.conversation,
                    ConversationIngressConversation::Create
                )),
                parent_conversation_ref: None,
                participant_actor_refs: Vec::new(),
                messages: Vec::new(),
                timeline_connection_ref: None,
                activity: Vec::new(),
            },
            ConversationIngressConversation::Continue { conversation_ref } => self
                .full_conversation(conversation_ref)?
                .ok_or_else(|| {
                    invalid_error(format!("unknown conversation ref {conversation_ref}"))
                })?,
        })
    }
}

fn validate_owner_context(
    ingress: &ConversationIngress,
    conversation: &RetainedConversation,
) -> CwrResult<()> {
    if ingress
        .owner_context_ref
        .as_deref()
        .is_some_and(|context| context.trim().is_empty())
    {
        return invalid("conversation owner context must not be blank");
    }
    if matches!(
        ingress.conversation,
        ConversationIngressConversation::Continue { .. }
    ) && ingress.owner_context_ref.is_some()
        && ingress.owner_context_ref != conversation.owner_context_ref
    {
        return invalid("an existing conversation cannot change owner context");
    }
    Ok(())
}
