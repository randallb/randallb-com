impl LocalRuntime {
    pub fn admit_ingress(
        &self,
        ingress: ConversationIngress,
        actor_integration_ref: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        self.admit_ingress_in_scope(ingress, actor_integration_ref, None)
    }

    pub fn admit_ingress_in_scope(
        &self,
        ingress: ConversationIngress,
        actor_integration_ref: Option<&str>,
        owner: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        self.admit_ingress_with_readiness(ingress, actor_integration_ref, owner, |route| {
            route.readiness()
        })
    }

    fn admit_ingress_with_source_input(
        &self,
        ingress: ConversationIngress,
        actor_integration_ref: Option<&str>,
        owner: Option<&str>,
        source_input_fingerprint: Option<&str>,
        active_call: Option<(&AdapterCall, &ActionInvocationRecord)>,
    ) -> CwrResult<LocalAdmission> {
        self.admit_ingress_with_source_input_and_readiness(
            ingress,
            actor_integration_ref,
            owner,
            source_input_fingerprint,
            |route| route.readiness(),
            active_call,
        )
    }

    fn admit_ingress_with_readiness(
        &self,
        ingress: ConversationIngress,
        actor_integration_ref: Option<&str>,
        owner: Option<&str>,
        readiness: impl FnMut(&crate::adapter::ResolvedProcessAdapterRoute) -> AdapterReadiness,
    ) -> CwrResult<LocalAdmission> {
        self.admit_ingress_with_source_input_and_readiness(
            ingress,
            actor_integration_ref,
            owner,
            None,
            readiness,
            None,
        )
    }

    fn admit_ingress_with_source_input_and_readiness(
        &self,
        mut ingress: ConversationIngress,
        actor_integration_ref: Option<&str>,
        owner: Option<&str>,
        source_input_fingerprint: Option<&str>,
        mut readiness: impl FnMut(&crate::adapter::ResolvedProcessAdapterRoute) -> AdapterReadiness,
        active_call: Option<(&AdapterCall, &ActionInvocationRecord)>,
    ) -> CwrResult<LocalAdmission> {
        let (snapshot, preference_actor) = {
            let state = self.lock()?;
            if state.shutting_down {
                return invalid("local runtime is shutting down");
            }
            let graph = upstream_graph(&state, owner)?;
            if let Some((call, caller)) = active_call {
                validate_create_conversation_call(&state, &graph, call, caller, &ingress)?;
            }
            if matches!(
                ingress.conversation,
                ConversationIngressConversation::Create
                    | ConversationIngressConversation::CreateForAction
            ) && ingress.owner_context_ref.is_none()
            {
                ingress.owner_context_ref = state
                    .admission
                    .admission(&ingress.integration_ref, &ingress.provider_event_ref)
                    .and_then(|record| state.admission.conversation_header(&record.conversation_ref))
                    .and_then(|conversation| conversation.owner_context_ref.clone())
                    .or_else(|| {
                        state
                            .admission
                            .context_policy
                            .as_ref()
                            .map(|policy| policy.default_context_ref.clone())
                    });
            }
            if state
                .admission
                .admission(&ingress.integration_ref, &ingress.provider_event_ref)
                .is_none()
            {
                require_native_schedule_owner_ingress(&state, &ingress)?;
                state.admission.check_ingress_context(&graph, &ingress)?;
            }
            let actor = state.admission.message_preference_actor(
                &graph,
                &ingress,
                actor_integration_ref,
            )?;
            (graph, actor)
        };
        // Authentication can take seconds. Keep other conversations and stop
        // requests responsive while selecting a route from the owned snapshot.
        let preferred_result = match &preference_actor {
            Some(actor) => snapshot.preferred_message_binding_with(actor, |binding| {
                let route = binding.process_adapter_route()?;
                let readiness = readiness(&route);
                Ok(readiness)
            }),
            None => Ok(None),
        };
        let route_snapshot = match (&preference_actor, &preferred_result) {
            (Some(actor), Ok(Some(reference))) => {
                Some(snapshot.message_binding_route_snapshot(actor, reference)?)
            }
            _ => None,
        };
        let mut state = self.lock()?;
        if state.shutting_down {
            return invalid("local runtime is shutting down");
        }
        let graph = upstream_graph(&state, owner)?;
        if let Some((call, caller)) = active_call {
            validate_create_conversation_call(&state, &graph, call, caller, &ingress)?;
        }
        if state
            .admission
            .admission(&ingress.integration_ref, &ingress.provider_event_ref)
            .is_none()
        {
            require_native_schedule_owner_ingress(&state, &ingress)?;
            state.admission.check_ingress_context(&graph, &ingress)?;
        }
        let current_actor =
            state
                .admission
                .message_preference_actor(&graph, &ingress, actor_integration_ref)?;
        // A concurrent replay is allowed to use its recorded result, without
        // requiring the formerly selected provider to still be ready.
        let replayed = state
            .admission
            .admission(&ingress.integration_ref, &ingress.provider_event_ref)
            .is_some();
        let preferred = if replayed { None } else { preferred_result? };
        if !replayed {
            if current_actor != preference_actor {
                return invalid("message route changed; retry the request");
            }
            if let Some(actor_ref) = &current_actor {
                let current = graph
                    .actors
                    .iter()
                    .find(|actor| &actor.actor_ref == actor_ref)
                    .unwrap();
                let previous = snapshot
                    .actors
                    .iter()
                    .find(|actor| &actor.actor_ref == actor_ref)
                    .unwrap();
                if current.message_binding_preference != previous.message_binding_preference {
                    return invalid("message route changed; retry the request");
                }
            }
            if let Some(reference) = &preferred {
                let actor_ref = current_actor
                    .as_deref()
                    .ok_or_else(|| invalid_error("message route changed; retry the request"))?;
                let current_snapshot =
                    graph
                        .message_binding_route_snapshot(actor_ref, reference)
                        .map_err(|_| invalid_error("message route changed; retry the request"))?;
                if Some(current_snapshot) != route_snapshot {
                    return invalid("message route changed; retry the request");
                }
            }
        }
        let host_local_actor_refs = owner
            .map(|owner| {
                state.upstream.visible(Some(owner)).map(|routes| {
                    routes
                        .into_iter()
                        .map(|route| route.coworker.actor_ref)
                        .collect::<Vec<_>>()
                })
            })
            .transpose()?
            .unwrap_or_default();
        let outcome = state.admission.admit_host_local(
            &graph,
            &ingress,
            actor_integration_ref,
            &host_local_actor_refs,
            preferred.as_deref(),
            source_input_fingerprint,
        )?;
        self.sync_archive_reservations(&mut state)?;
        match outcome {
            ConversationAdmissionOutcome::Started(turn) => {
                if let (Some(invocation_ref), Some(owner)) =
                    (turn.record.invocation_ref.as_ref(), owner)
                {
                    state
                        .host_local_invocation_owners
                        .insert(invocation_ref.clone(), owner.to_owned());
                }
                let conversation = state
                    .admission
                    .conversation(&turn.record.conversation_ref)
                    .cloned()
                    .ok_or_else(|| invalid_error("retained admission has no conversation"))?;
                Ok(LocalAdmission {
                    record: turn.record,
                    conversation,
                })
            }
            ConversationAdmissionOutcome::Existing(record) => {
                if let (Some(invocation_ref), Some(owner)) = (record.invocation_ref.as_ref(), owner)
                {
                    state
                        .host_local_invocation_owners
                        .entry(invocation_ref.clone())
                        .or_insert_with(|| owner.to_owned());
                }
                let conversation = state
                    .admission
                    .full_conversation(&record.conversation_ref)?
                    .ok_or_else(|| invalid_error("retained admission has no conversation"))?;
                Ok(LocalAdmission {
                    record: *record,
                    conversation,
                })
            }
        }
    }
}

fn require_native_schedule_owner_ingress(
    state: &LocalRuntimeState,
    ingress: &ConversationIngress,
) -> CwrResult<()> {
    let (
        Some(owner),
        ConversationIngressConversation::CreateForAction,
        ConversationIngressAddressing::Action { activation, .. },
    ) = (
        ingress.owner_context_ref.as_deref(),
        &ingress.conversation,
        &ingress.addressing,
    )
    else {
        return Ok(());
    };
    if matches!(
        activation.as_str(),
        "@cwr--list--schedules"
            | "@cwr--get--schedule"
            | "@cwr--pause--schedule"
            | "@cwr--resume--schedule"
            | "@cwr--list--schedule_runs"
    ) {
        require_installed_schedule_owner_in_state(state, owner, &ingress.author_actor_ref)?;
    }
    Ok(())
}
