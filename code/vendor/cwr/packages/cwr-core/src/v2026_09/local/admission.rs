impl LocalRuntime {
    pub fn record_host_exchange(
        &self,
        conversation_ref: Option<&str>,
        requesting_actor_ref: &str,
        host_actor_ref: &str,
        request_text: String,
        response_text: String,
    ) -> CwrResult<crate::v2026_09::conversation::HostConversationExchange> {
        let mut state = self.lock()?;
        if state.shutting_down {
            return invalid("local runtime is shutting down");
        }
        if !state
            .graph
            .actors
            .iter()
            .any(|actor| actor.actor_ref == requesting_actor_ref)
        {
            return invalid(format!(
                "host exchange requester {requesting_actor_ref} is not in the resolved graph"
            ));
        }
        if state
            .graph
            .actors
            .iter()
            .any(|actor| actor.actor_ref == host_actor_ref)
        {
            return invalid(format!(
                "host exchange actor {host_actor_ref} collides with a resolved graph actor"
            ));
        }
        let exchange = state.admission.record_host_exchange(
            conversation_ref,
            requesting_actor_ref,
            host_actor_ref,
            request_text,
            response_text,
        )?;
        Ok(exchange)
    }

    /// Admits local text through the runtime catalog. The runtime-provided
    /// `@cwr` actor follows the same retained conversation path as other actors.
    pub fn submit_text(
        &self,
        conversation_ref: Option<&str>,
        text: impl Into<String>,
    ) -> CwrResult<LocalTextOutcome> {
        self.submit_text_for_host(conversation_ref, text, &LocalIntrinsicCwrHost::default())
    }

    /// Handles local text with the optional capabilities and coworkers
    /// projected by a host.
    pub fn submit_text_for_host(
        &self,
        conversation_ref: Option<&str>,
        text: impl Into<String>,
        _host: &LocalIntrinsicCwrHost,
    ) -> CwrResult<LocalTextOutcome> {
        let text = text.into();
        if text.trim().is_empty() {
            return invalid("local conversation message must not be blank");
        }
        self.admit_text(conversation_ref, text)
            .map(Box::new)
            .map(LocalTextOutcome::Conversation)
    }

    /// Parses and dispatches intrinsic text without admitting ordinary text.
    pub fn dispatch_intrinsic_text_for_host(
        &self,
        text: &str,
        host: &LocalIntrinsicCwrHost,
    ) -> CwrResult<Option<IntrinsicCwrDispatch>> {
        let Some(command) = parse_intrinsic_cwr_command(text) else {
            return Ok(None);
        };
        self.dispatch_intrinsic_command_for_host(&command, host)
            .map(Some)
    }

    /// Dispatches one already-parsed intrinsic command through the runtime's
    /// current catalog and the supplied host projection.
    pub fn dispatch_intrinsic_command_for_host(
        &self,
        command: &IntrinsicCwrCommand,
        host: &LocalIntrinsicCwrHost,
    ) -> CwrResult<IntrinsicCwrDispatch> {
        let acting_actor_ref = self.catalog()?.acting_actor.actor_ref;
        self.dispatch_intrinsic_command_as_for_host(&acting_actor_ref, command, host)
    }

    /// Dispatches an intrinsic command through one connected actor's catalog.
    pub fn dispatch_intrinsic_command_as_for_host(
        &self,
        acting_actor_ref: &str,
        command: &IntrinsicCwrCommand,
        host: &LocalIntrinsicCwrHost,
    ) -> CwrResult<IntrinsicCwrDispatch> {
        self.dispatch_intrinsic_command_as_in_conversation_for_host(
            None,
            acting_actor_ref,
            command,
            host,
        )
    }

    pub fn dispatch_intrinsic_command_as_in_conversation_for_host(
        &self,
        conversation_ref: Option<&str>,
        acting_actor_ref: &str,
        command: &IntrinsicCwrCommand,
        host: &LocalIntrinsicCwrHost,
    ) -> CwrResult<IntrinsicCwrDispatch> {
        if let Some(mut response) =
            self.discovery_as(conversation_ref, acting_actor_ref, None, command)?
        {
            match &mut response {
                IntrinsicCwrResponse::Coworkers { coworkers } => {
                    let allowed = crate::intrinsic_cwr::intrinsic_cwr_actions(host.capabilities);
                    for coworker in coworkers.iter_mut() {
                        coworker.actions.retain(|a| {
                            !a.starts_with("@cwr--")
                                || a == "@cwr--invite--coworker"
                                || allowed.contains(a)
                        });
                    }
                    coworkers.extend(host.additional_coworkers.iter().cloned());
                }
                IntrinsicCwrResponse::Actions { actions, .. }
                | IntrinsicCwrResponse::Help { actions } => {
                    let allowed = crate::intrinsic_cwr::intrinsic_cwr_actions(host.capabilities);
                    actions.retain(|a| {
                        !a.starts_with("@cwr--")
                            || a == "@cwr--invite--coworker"
                            || allowed.contains(a)
                    });
                    actions.extend(
                        host.additional_coworkers
                            .iter()
                            .flat_map(|coworker| coworker.actions.iter().cloned()),
                    );
                    actions.sort();
                    actions.dedup();
                }
                _ => {}
            }
            if let IntrinsicCwrResponse::Actions {
                actions,
                input_schemas,
                descriptions,
                targets,
            } = &mut response
            {
                targets.retain(|activation, _| actions.contains(activation));
                descriptions.retain(|activation, _| actions.contains(activation));
                input_schemas.retain(|activation, _| actions.contains(activation));
                for activation in actions {
                    input_schemas
                        .entry(activation.clone())
                        .or_insert_with(|| json!({}));
                }
            }
            return Ok(IntrinsicCwrDispatch::Response(response));
        }
        let mut catalog = self.catalog_as(acting_actor_ref)?.intrinsic_cwr_catalog();
        catalog
            .coworkers
            .extend(host.additional_coworkers.iter().cloned());
        Ok(execute_intrinsic_cwr_command(
            command,
            &catalog,
            host.capabilities,
        ))
    }

    /// Explicitly bypasses intrinsic CWR handling and admits conversation text.
    /// Hosts should normally use [`Self::submit_text`] instead.
    pub fn admit_conversation_text(
        &self,
        conversation_ref: Option<&str>,
        text: impl Into<String>,
    ) -> CwrResult<LocalAdmission> {
        self.admit_text(conversation_ref, text)
    }

    fn admit_text(
        &self,
        conversation_ref: Option<&str>,
        text: impl Into<String>,
    ) -> CwrResult<LocalAdmission> {
        let text = text.into();
        if text.trim().is_empty() {
            return invalid("local conversation message must not be blank");
        }
        let (addressing, actor_integration_ref, message_text) = {
            let state = self.lock()?;
            let (addressing, mut binding, text) = local_addressing(&state.catalog, &text)?;
            if let ConversationIngressAddressing::Addressed { actor_refs } = &addressing {
                if state.graph.actors.iter().any(|actor| {
                    actor_refs.contains(&actor.actor_ref)
                        && !actor.message_binding_preference.is_empty()
                }) {
                    binding = None;
                }
            }
            (addressing, binding, text)
        };
        self.admit(
            conversation_ref,
            message_text,
            addressing,
            actor_integration_ref,
        )
    }

    pub fn admit_action(
        &self,
        conversation_ref: Option<&str>,
        activation: &str,
        input: Value,
    ) -> CwrResult<LocalAdmission> {
        let (integration_ref, author_actor_ref) = {
            let state = self.lock()?;
            (
                state.host_identity.ingress_integration_ref.clone(),
                state.host_identity.acting_actor_ref.clone(),
            )
        };
        self.admit_action_as(
            integration_ref,
            format!("local-{}", Uuid::new_v4().simple()),
            author_actor_ref,
            conversation_ref,
            activation,
            input,
        )
    }

    pub fn admit_action_as(
        &self,
        integration_ref: impl Into<String>,
        request_id: impl Into<String>,
        author_actor_ref: impl Into<String>,
        conversation_ref: Option<&str>,
        activation: &str,
        input: Value,
    ) -> CwrResult<LocalAdmission> {
        self.admit_action_in_upstream_scope(
            integration_ref,
            request_id,
            author_actor_ref,
            conversation_ref,
            activation,
            input,
            None,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "The connected host supplies one explicit session scope alongside ordinary admission fields."
    )]
    pub fn admit_action_in_upstream_scope(
        &self,
        integration_ref: impl Into<String>,
        request_id: impl Into<String>,
        author_actor_ref: impl Into<String>,
        conversation_ref: Option<&str>,
        activation: &str,
        input: Value,
        owner: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        self.admit_action_in_context(
            integration_ref,
            request_id,
            author_actor_ref,
            conversation_ref,
            activation,
            input,
            owner,
            None,
        )
    }

    /// The native UI selects an installed owner as the action destination. Only
    /// schedule controls may use this path; the owner is never action input.
    #[expect(
        clippy::too_many_arguments,
        reason = "The authenticated host supplies an owner destination and an upstream session scope."
    )]
    pub fn admit_owner_schedule_action_in_upstream_scope(
        &self,
        integration_ref: impl Into<String>,
        request_id: impl Into<String>,
        author_actor_ref: impl Into<String>,
        owner_context_ref: &str,
        activation: &str,
        input: Value,
        owner: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        if !matches!(
            activation,
            "@cwr--list--schedules"
                | "@cwr--get--schedule"
                | "@cwr--pause--schedule"
                | "@cwr--resume--schedule"
                | "@cwr--list--schedule_runs"
        ) {
            return invalid("only schedule controls accept an installed owner destination");
        }
        self.admit_action_in_context(
            integration_ref,
            request_id,
            author_actor_ref,
            None,
            activation,
            input,
            owner,
            Some(owner_context_ref),
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Action admission carries the authenticated host scope and optional installed owner destination."
    )]
    fn admit_action_in_context(
        &self,
        integration_ref: impl Into<String>,
        request_id: impl Into<String>,
        author_actor_ref: impl Into<String>,
        conversation_ref: Option<&str>,
        activation: &str,
        input: Value,
        owner: Option<&str>,
        owner_context_ref: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        let integration_ref = integration_ref.into();
        let request_id = request_id.into();
        let author_actor_ref = author_actor_ref.into();
        // Authenticate the current caller even when recovering removed actions.
        let catalog = self.upstream_catalog_as(&author_actor_ref, owner)?;
        if let Some(admission) = self.recover_action_request(
            &integration_ref,
            &request_id,
            &author_actor_ref,
            conversation_ref,
            activation,
            &input,
            owner_context_ref,
        )? {
            return Ok(admission);
        }
        if let Some(owner_context_ref) = owner_context_ref {
            self.require_installed_schedule_owner(owner_context_ref, &author_actor_ref)?;
        }
        let action = catalog
            .action(activation)
            .cloned()
            .ok_or_else(|| invalid_error(format!("unknown action activation {activation}")))?;
        let text = if input.as_object().is_some_and(Map::is_empty) {
            activation.to_owned()
        } else {
            format!("{activation} {}", serde_json::to_string(&input)?)
        };
        self.admit_ingress_in_scope(
            ConversationIngress {
                owner_context_ref: owner_context_ref.map(str::to_owned),
                file_refs: Vec::new(),
                integration_ref,
                provider_event_ref: request_id,
                author_actor_ref,
                conversation: conversation_ref.map_or(
                    ConversationIngressConversation::CreateForAction,
                    |conversation_ref| ConversationIngressConversation::Continue {
                        conversation_ref: conversation_ref.into(),
                    },
                ),
                addressing: ConversationIngressAddressing::Action {
                    actor_ref: action.actor_ref,
                    activation: activation.into(),
                    action_name: action.action_name,
                    input,
                },
                text,
            },
            Some(&action.actor_integration_ref),
            owner,
        )
    }

    /// Admits one host-authenticated gateway entry point without adding the
    /// standalone handler to ordinary coworker action discovery.
    #[expect(
        clippy::too_many_arguments,
        reason = "A connected gateway supplies its exact registered contract and host event fields."
    )]
    pub fn admit_gateway_action_in_upstream_scope(
        &self,
        connection_ref: &str,
        integration_ref: impl Into<String>,
        request_id: impl Into<String>,
        author_actor_ref: impl Into<String>,
        conversation_ref: Option<&str>,
        activation: &str,
        input: Value,
        owner: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        let integration_ref = integration_ref.into();
        let request_id = request_id.into();
        let author_actor_ref = author_actor_ref.into();
        // The author still must be a runtime actor visible to this host.
        self.upstream_catalog_as(&author_actor_ref, owner)?;
        let graph = {
            let state = self.lock()?;
            upstream_graph(&state, owner)?
        };
        let gateway = graph
            .gateways
            .iter()
            .find(|gateway| gateway.connection_ref.as_deref() == Some(connection_ref))
            .ok_or_else(|| invalid_error("approved gateway definition is unavailable"))?;
        let route = [
            gateway.ingress_route.as_ref(),
            gateway.delivery_route.as_ref(),
        ]
        .into_iter()
        .flatten()
        .find(|route| activation == format!("gateway--{}", route.action_name))
        .ok_or_else(|| invalid_error("gateway activation differs from approved entry points"))?;
        route.require_available()?;
        route.validate_input(&input)?;
        let text = if input.as_object().is_some_and(Map::is_empty) {
            activation.to_owned()
        } else {
            format!("{activation} {}", serde_json::to_string(&input)?)
        };
        self.admit_ingress_in_scope(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref,
                provider_event_ref: request_id,
                author_actor_ref,
                conversation: conversation_ref.map_or(
                    ConversationIngressConversation::CreateForAction,
                    |conversation_ref| ConversationIngressConversation::Continue {
                        conversation_ref: conversation_ref.into(),
                    },
                ),
                addressing: ConversationIngressAddressing::Action {
                    actor_ref: route.actor_ref.clone(),
                    activation: activation.into(),
                    action_name: route.action_name.clone(),
                    input,
                },
                text,
            },
            Some(&route.actor_integration_ref),
            owner,
        )
    }

    fn admit(
        &self,
        conversation_ref: Option<&str>,
        text: String,
        addressing: ConversationIngressAddressing,
        actor_integration_ref: Option<String>,
    ) -> CwrResult<LocalAdmission> {
        let (integration_ref, author_actor_ref) = {
            let state = self.lock()?;
            (
                state.host_identity.ingress_integration_ref.clone(),
                state.host_identity.acting_actor_ref.clone(),
            )
        };
        self.admit_ingress(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref,
                provider_event_ref: format!("local-{}", Uuid::new_v4().simple()),
                author_actor_ref,
                conversation: conversation_ref.map_or(
                    ConversationIngressConversation::Create,
                    |conversation_ref| ConversationIngressConversation::Continue {
                        conversation_ref: conversation_ref.into(),
                    },
                ),
                addressing,
                text,
            },
            actor_integration_ref.as_deref(),
        )
    }
}
