impl LocalRuntime {
    pub fn submit_text_as(
        &self,
        integration_ref: impl Into<String>,
        request_id: impl Into<String>,
        author_actor_ref: impl Into<String>,
        conversation_ref: Option<&str>,
        text: impl Into<String>,
    ) -> CwrResult<LocalAdmission> {
        self.submit_text_in_upstream_scope(
            integration_ref,
            request_id,
            author_actor_ref,
            conversation_ref,
            text,
            None,
        )
    }

    pub fn submit_text_in_upstream_scope(
        &self,
        integration_ref: impl Into<String>,
        request_id: impl Into<String>,
        author_actor_ref: impl Into<String>,
        conversation_ref: Option<&str>,
        text: impl Into<String>,
        owner: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        self.submit_message_in_upstream_scope(
            integration_ref,
            request_id,
            author_actor_ref,
            conversation_ref,
            text,
            Vec::new(),
            owner,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Message admission carries the authenticated scope and exact retry input."
    )]
    pub fn submit_message_in_upstream_scope(
        &self,
        integration_ref: impl Into<String>,
        request_id: impl Into<String>,
        author_actor_ref: impl Into<String>,
        conversation_ref: Option<&str>,
        text: impl Into<String>,
        file_refs: Vec<String>,
        owner: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        self.submit_message_in_context(
            integration_ref,
            request_id,
            author_actor_ref,
            conversation_ref,
            text,
            file_refs,
            owner,
            None,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Carries authenticated host scope and exact durable admission input."
    )]
    pub fn submit_message_in_context(
        &self,
        integration_ref: impl Into<String>,
        request_id: impl Into<String>,
        author_actor_ref: impl Into<String>,
        conversation_ref: Option<&str>,
        text: impl Into<String>,
        file_refs: Vec<String>,
        owner: Option<&str>,
        owner_context_ref: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        self.submit_message_from_call(
            integration_ref,
            request_id,
            author_actor_ref,
            conversation_ref,
            text,
            file_refs,
            owner,
            owner_context_ref,
            None,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Carries exact admission input plus the active coworker call authority."
    )]
    fn submit_message_from_call(
        &self,
        integration_ref: impl Into<String>,
        request_id: impl Into<String>,
        author_actor_ref: impl Into<String>,
        conversation_ref: Option<&str>,
        text: impl Into<String>,
        file_refs: Vec<String>,
        owner: Option<&str>,
        owner_context_ref: Option<&str>,
        active_call: Option<(&AdapterCall, &ActionInvocationRecord)>,
    ) -> CwrResult<LocalAdmission> {
        let integration_ref = integration_ref.into();
        let request_id = request_id.into();
        let author_actor_ref = author_actor_ref.into();
        let text = text.into();
        if text.trim().is_empty() && file_refs.is_empty() {
            return invalid("conversation message must not be blank");
        }
        let mut source_input_fingerprint = conversation_message_source_fingerprint(
            &author_actor_ref,
            conversation_ref,
            &text,
            &file_refs,
        )?;
        if let Some(context) = owner_context_ref {
            let state = self.lock()?;
            if let Some(record) = state.admission.admission(&integration_ref, &request_id) {
                let retained = state
                    .admission
                    .conversation_header(&record.conversation_ref)
                    .and_then(|conversation| conversation.owner_context_ref.as_deref());
                if retained != Some(context) {
                    return Err(context_scope_error(
                        "request ID was reused with a different owner context",
                    ));
                }
            }
            source_input_fingerprint = format!(
                "sha256:{}",
                hex::encode(Sha256::digest(serde_json::to_vec(&(
                    &source_input_fingerprint,
                    context
                ))?))
            );
        }
        if let Some(admission) = self.retained_text_admission(
            &integration_ref,
            &request_id,
            &author_actor_ref,
            conversation_ref,
            &text,
            &file_refs,
            &source_input_fingerprint,
            owner,
        )? {
            return Ok(admission);
        }
        let (addressing, actor_integration_ref, message_text) = {
            let state = self.lock()?;
            let mut catalog = local_catalog(&upstream_graph(&state, owner)?, &author_actor_ref)?;
            if let Some(policy) = state.admission.context_policy.as_ref() {
                let context = conversation_ref
                    .and_then(|reference| state.admission.conversation_header(reference))
                    .and_then(|c| c.owner_context_ref.as_deref())
                    .or(owner_context_ref)
                    .unwrap_or(&policy.default_context_ref);
                let allowed = policy
                    .contexts
                    .get(context)
                    .map(Vec::as_slice).unwrap_or_default();
                let visible = message_target_identities(&state, &author_actor_ref, owner)?;
                catalog.default_conversation_target_ref = allowed
                    .iter()
                    .find(|actor| visible.iter().any(|known| &known.actor_ref == *actor))
                    .cloned();
            }
            let (addressing, mut binding, text) =
                local_addressing_with_files(&catalog, &text, !file_refs.is_empty())?;
            if let ConversationIngressAddressing::Addressed { actor_refs } = &addressing {
                // The ordinary catalog excludes its author as a message target.
                // An authenticated creation call may explicitly start fresh work
                // for itself in another root, without resuming its active turn.
                if active_call.is_some() && conversation_ref.is_none()
                    && actor_refs.as_slice() == [author_actor_ref.as_str()]
                {
                    binding = upstream_graph(&state, owner)?.actors.iter()
                        .find(|actor| actor.actor_ref == author_actor_ref)
                        .and_then(|actor| actor.default_actor_integration_ref.clone());
                }
                if state.graph.actors.iter().any(|actor| {
                    actor_refs.contains(&actor.actor_ref)
                        && !actor.message_binding_preference.is_empty()
                }) {
                    binding = None;
                }
            }
            (addressing, binding, text)
        };
        self.admit_ingress_with_source_input(
            ConversationIngress {
                owner_context_ref: owner_context_ref.map(str::to_owned),
                file_refs,
                integration_ref,
                provider_event_ref: request_id,
                author_actor_ref,
                conversation: conversation_ref.map_or(
                    ConversationIngressConversation::Create,
                    |conversation_ref| ConversationIngressConversation::Continue {
                        conversation_ref: conversation_ref.into(),
                    },
                ),
                addressing,
                text: message_text,
            },
            actor_integration_ref.as_deref(),
            owner,
            Some(&source_input_fingerprint),
            active_call,
        )
    }

    pub fn replay_text_in_upstream_scope(
        &self,
        integration_ref: impl Into<String>,
        request_id: impl Into<String>,
        author_actor_ref: impl Into<String>,
        conversation_ref: &str,
        text: impl Into<String>,
        owner: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        let integration_ref = integration_ref.into();
        let request_id = request_id.into();
        let author_actor_ref = author_actor_ref.into();
        let text = text.into();
        let source_input_fingerprint =
            conversation_text_source_fingerprint(&author_actor_ref, Some(conversation_ref), &text)?;
        self.retained_text_admission(
            &integration_ref,
            &request_id,
            &author_actor_ref,
            Some(conversation_ref),
            &text,
            &[],
            &source_input_fingerprint,
            owner,
        )?
        .ok_or_else(|| {
            invalid_error(format!(
                "conversation ingress event {request_id} has no retained admission"
            ))
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Replay identity is the complete durable text-admission boundary."
    )]
    fn retained_text_admission(
        &self,
        integration_ref: &str,
        request_id: &str,
        author_actor_ref: &str,
        conversation_ref: Option<&str>,
        text: &str,
        file_refs: &[String],
        source_input_fingerprint: &str,
        owner: Option<&str>,
    ) -> CwrResult<Option<LocalAdmission>> {
        let mut state = self.lock()?;
        let Some(mut record) = state
            .admission
            .admission(integration_ref, request_id)
            .cloned()
        else {
            return Ok(None);
        };
        match record.source_input_fingerprint.as_deref() {
            Some(retained_fingerprint) if retained_fingerprint != source_input_fingerprint => {
                return invalid(format!(
                    "conversation ingress event {request_id} was reused with different source input"
                ));
            }
            None => {
                let retained_target_handle = match record.target_actor_ref.as_deref() {
                    Some(target_actor_ref) => {
                        let upstream = state
                            .admission
                            .retained_upstream_actor(&record.conversation_ref, target_actor_ref)?
                            .map(|actor| actor.handle)
                            .or(state
                                .admission
                                .retained_upstream_actor_handle(target_actor_ref)?);
                        upstream.or_else(|| {
                            state
                                .catalog
                                .retained_actors
                                .iter()
                                .find(|actor| actor.actor_ref == target_actor_ref)
                                .map(|actor| actor.handle.clone())
                        })
                    }
                    None => None,
                };
                let verified = state
                    .admission
                    .full_conversation(&record.conversation_ref)?
                    .as_ref()
                    .is_some_and(|conversation| {
                        legacy_replay_source_matches(
                            &record,
                            conversation,
                            author_actor_ref,
                            conversation_ref,
                            text,
                            file_refs,
                            retained_target_handle.as_deref(),
                        )
                    });
                if !verified {
                    return invalid(format!(
                        "conversation ingress event {request_id} was reused with different source input"
                    ));
                }
                record = state.admission.record_source_input_fingerprint(
                    integration_ref,
                    request_id,
                    source_input_fingerprint,
                )?;
            }
            Some(_) => {}
        }
        if let (Some(invocation_ref), Some(owner)) = (record.invocation_ref.as_ref(), owner) {
            state
                .host_local_invocation_owners
                .entry(invocation_ref.clone())
                .or_insert_with(|| owner.to_owned());
        }
        let conversation = state
            .admission
            .full_conversation(&record.conversation_ref)?
            .ok_or_else(|| invalid_error("retained admission has no conversation"))?;
        Ok(Some(LocalAdmission {
            record,
            conversation,
        }))
    }
}

fn legacy_replay_source_matches(
    record: &ConversationAdmissionRecord,
    conversation: &RetainedConversation,
    author_actor_ref: &str,
    conversation_ref: Option<&str>,
    text: &str,
    file_refs: &[String],
    retained_target_handle: Option<&str>,
) -> bool {
    let Some((message_index, message)) = conversation
        .messages
        .iter()
        .enumerate()
        .find(|(_, message)| message.message_ref == record.source_message_ref)
    else {
        return false;
    };
    if !legacy_replay_conversation_matches(record, message, message_index, conversation_ref) {
        return false;
    }
    if message.author_actor_ref != author_actor_ref || message.file_refs != file_refs {
        return false;
    }
    if message.action.is_some()
        || message.addressing_mode
            != Some(crate::v2026_09::conversation::ConversationAddressingMode::Explicit)
    {
        return message.text == text;
    }
    if !record
        .target_actor_ref
        .as_ref()
        .is_some_and(|target| message.addressed_actor_refs.contains(target))
    {
        return false;
    }
    let trimmed = text.trim();
    let leading = trimmed.split_ascii_whitespace().next().unwrap_or_default();
    legacy_target_handle_matches(retained_target_handle, leading)
        && trimmed[leading.len()..].trim() == message.text
}

fn legacy_replay_conversation_matches(
    record: &ConversationAdmissionRecord,
    message: &ConversationMessage,
    message_index: usize,
    conversation_ref: Option<&str>,
) -> bool {
    let conversation = match conversation_ref {
        Some(conversation_ref) if record.conversation_ref == conversation_ref => {
            ConversationIngressConversation::Continue {
                conversation_ref: conversation_ref.into(),
            }
        }
        Some(_) => return false,
        None => ConversationIngressConversation::Create,
    };
    let retained_fingerprint = record.ingress_fingerprint.as_deref();
    let addressing = match (
        message.action.as_ref(),
        message.addressing_mode,
        record.target_actor_ref.as_ref(),
    ) {
        (
            None,
            Some(crate::v2026_09::conversation::ConversationAddressingMode::Explicit),
            Some(target),
        ) => Some(ConversationIngressAddressing::Addressed {
            actor_refs: vec![target.clone()],
        }),
        (None, Some(crate::v2026_09::conversation::ConversationAddressingMode::Implicit), None) => {
            Some(ConversationIngressAddressing::Unaddressed)
        }
        _ => None,
    };
    if let (Some(retained_fingerprint), Some(addressing)) = (retained_fingerprint, addressing) {
        let ingress = ConversationIngress {
            owner_context_ref: None,
            file_refs: message.file_refs.clone(),
            integration_ref: record.integration_ref.clone(),
            provider_event_ref: record.provider_event_ref.clone(),
            author_actor_ref: message.author_actor_ref.clone(),
            conversation,
            addressing,
            text: message.text.clone(),
        };
        let Ok(encoded) = serde_json::to_vec(&ingress) else {
            return false;
        };
        let candidate = format!("sha256:{}", hex::encode(Sha256::digest(encoded)));
        return candidate == retained_fingerprint;
    }
    match conversation_ref {
        Some(_) => message_index > 0,
        None => message_index == 0,
    }
}

fn legacy_target_handle_matches(retained: Option<&str>, replayed: &str) -> bool {
    match retained {
        Some(retained) if retained.eq_ignore_ascii_case("@cwr") => {
            replayed.eq_ignore_ascii_case("@cwr")
        }
        Some(retained) => retained == replayed,
        None => false,
    }
}

#[derive(Serialize)]
struct ConversationTextSourceInput<'a> {
    author_actor_ref: &'a str,
    conversation_ref: Option<&'a str>,
    text: &'a str,
}

fn conversation_text_source_fingerprint(
    author_actor_ref: &str,
    conversation_ref: Option<&str>,
    text: &str,
) -> CwrResult<String> {
    let encoded = serde_json::to_vec(&ConversationTextSourceInput {
        author_actor_ref,
        conversation_ref,
        text,
    })?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(encoded))))
}

fn conversation_message_source_fingerprint(
    author: &str,
    conversation: Option<&str>,
    text: &str,
    files: &[String],
) -> CwrResult<String> {
    if files.is_empty() {
        return conversation_text_source_fingerprint(author, conversation, text);
    }
    let encoded = serde_json::to_vec(&(author, conversation, text, files))?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(encoded))))
}

impl LocalRuntime {
    pub fn ingest_file_as(
        &self,
        actor_ref: &str,
        filename: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> CwrResult<crate::v2026_09::conversation::RetainedFile> {
        let mut state = self.lock()?;
        if state.shutting_down {
            return invalid("local runtime is shutting down");
        }
        state
            .admission
            .ingest_file(actor_ref, filename, media_type, bytes)
    }
    pub fn file_metadata_as(
        &self,
        actor_ref: &str,
        reference: &str,
    ) -> CwrResult<crate::v2026_09::conversation::RetainedFile> {
        self.lock()?.admission.file_metadata(actor_ref, reference)
    }
    pub fn read_file_as(
        &self,
        actor_ref: &str,
        reference: &str,
    ) -> CwrResult<crate::v2026_09::conversation::RetainedFileContent> {
        self.lock()?.admission.read_file(actor_ref, reference)
    }
}
