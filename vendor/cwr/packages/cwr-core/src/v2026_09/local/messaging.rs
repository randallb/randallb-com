struct MessagePreflightReadiness {
    binding_ref: String,
    process: Option<(String, AdapterReadiness)>,
    preference: Option<Vec<String>>,
    route_snapshot: Option<Vec<(String, bool, Option<String>)>>,
}

const SEND_MESSAGE_HOST_OPERATION: &str = "cwr.send_message";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendMessageInput {
    #[serde(alias = "loop_ref")]
    runtime_ref: Option<String>,
    binding_ref: Option<String>,
    to: String,
    text: String,
}

fn send_message_operation() -> AdapterHostOperation {
    AdapterHostOperation {
        name: SEND_MESSAGE_HOST_OPERATION.into(),
        description: "Ask a loaded coworker and wait for its reply. Use its exact coworker ref or an unambiguous handle. Accepted requests establish participation.".into(),
        input_schema: json!({"type":"object", "additionalProperties":false,
            "required":["to","text"], "properties": {
                "to":{"type":"string","minLength":1},
                "runtime_ref":{"type":"string"}, "binding_ref":{"type":"string"},
                "text":{"type":"string","minLength":1}}}),
        output_schema: json!({"type":"object"}),
    }
}

fn validate_message_preflight(
    graph: &ResolvedCwrGraph,
    binding: &ResolvedCwrActorIntegration,
    preflight: &MessagePreflightReadiness,
    current_preference: &[String],
) -> CwrResult<()> {
    if preflight.binding_ref != binding.actor_integration_ref
        || preflight
            .preference
            .as_ref()
            .is_some_and(|saved| saved != current_preference)
    {
        return invalid("message route changed; retry the request");
    }
    if let Some(previous) = &preflight.route_snapshot {
        let current = graph
            .message_binding_route_snapshot(&binding.actor_ref, &preflight.binding_ref)
            .map_err(|_| invalid_error("message route changed; retry the request"))?;
        if &current != previous {
            return invalid("message route changed; retry the request");
        }
    }
    let Some((preflight_fingerprint, readiness)) = &preflight.process else {
        return if binding.adapter_ref == UPSTREAM_ADAPTER_REF {
            Ok(())
        } else {
            invalid("message route changed; retry the request")
        };
    };
    if binding.adapter_ref == UPSTREAM_ADAPTER_REF
        || preflight_fingerprint != &binding.process_adapter_route()?.fingerprint()
    {
        return invalid("message route changed; retry the request");
    }
    if !readiness.is_ready() {
        return invalid(readiness.message.clone());
    }
    Ok(())
}

impl LocalRuntime {
    fn message_preflight_readiness(
        &self,
        input: &SendMessageInput,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
    ) -> CwrResult<Option<MessagePreflightReadiness>> {
        self.message_preflight_with_readiness(input, call, caller, |route| route.readiness())
    }

    fn message_preflight_with_readiness(
        &self,
        input: &SendMessageInput,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
        mut readiness: impl FnMut(&crate::adapter::ResolvedProcessAdapterRoute) -> AdapterReadiness,
    ) -> CwrResult<Option<MessagePreflightReadiness>> {
        let state = self.lock()?;
        if input
            .runtime_ref
            .as_ref()
            .is_some_and(|reference| reference != &state.graph.runtime_ref)
        {
            return invalid("cross-runtime collaboration is not supported");
        }
        require_no_stop_fence(&state, &call.invocation_ref)?;
        require_active_caller(&state.admission, call)?;
        if state.shutting_down {
            return invalid("local runtime is shutting down");
        }
        let existing = state.admission.invocation_records().any(|record| {
            matches!(
                &record.origin, InvocationOrigin::HostCall { caller_invocation_ref, call_id }
                if caller_invocation_ref == &call.invocation_ref && call_id == &call.call_id
            )
        });
        if existing {
            return Ok(None);
        }
        let owner = state
            .host_local_invocation_owners
            .get(&caller.invocation_ref);
        let graph = upstream_graph(&state, owner.map(String::as_str))?;
        let catalog = local_catalog(&graph, &caller.target_actor_ref)?;
        let target = catalog
            .known_actor_by_ref(&input.to)
            .or_else(|| catalog.known_actor_by_handle(&input.to))
            .ok_or_else(|| invalid_error(format!("unknown participant {}", input.to)))?;
        let conversation = state.admission.working_conversation_ref(caller)?;
        let context = state.admission.conversation_header(conversation).and_then(|conversation| conversation.owner_context_ref.as_deref());
        state.admission.check_context_work(
            &graph,
            context,
            true,
            &caller.target_actor_ref,
            Some((&target.actor_ref, Some(RESPOND_TO_MESSAGE_ACTION_NAME))),
            Some(caller),
        )?;
        if target.actor_ref == caller.target_actor_ref {
            return invalid("send_message addresses another coworker, not the caller");
        }
        let allowed = action_actor_allowlist(
            &graph,
            &caller.target_actor_ref,
            caller.actor_integration_ref.as_deref(),
        )?;
        if allowed
            .as_ref()
            .is_some_and(|refs| !refs.contains(&target.actor_ref))
        {
            return invalid("coworker request is not authorized");
        }
        let Some(actor) = catalog.actor_by_ref(&target.actor_ref) else {
            return Ok(None);
        };
        let preference = input.binding_ref.is_none().then(|| {
            graph
                .actors
                .iter()
                .find(|actor| actor.actor_ref == target.actor_ref)
                .map(|actor| actor.message_binding_preference.clone())
                .unwrap_or_default()
        });
        // The graph/catalog are owned snapshots. Provider processes must never
        // hold the runtime mutex; admission revalidates the selected fingerprint.
        drop(state);
        let mut checked = BTreeMap::new();
        let preferred = if input.binding_ref.is_none() {
            graph.preferred_message_binding_with(&target.actor_ref, |binding| {
                let route = binding.process_adapter_route()?;
                let status = readiness(&route);
                checked.insert(
                    binding.actor_integration_ref.clone(),
                    (route.fingerprint(), status.clone()),
                );
                Ok(status)
            })?
        } else {
            None
        };
        let selected_binding = input
            .binding_ref
            .as_deref()
            .or(preferred.as_deref())
            .unwrap_or(&actor.actor_integration_ref);
        let binding = graph
            .actor_integrations
            .iter()
            .find(|binding| {
                binding.actor_ref == target.actor_ref
                    && binding.actor_integration_ref == selected_binding
            })
            .ok_or_else(|| invalid_error("message route is unavailable"))?;
        binding.require_available()?;
        let route_snapshot = preferred
            .as_deref()
            .map(|reference| graph.message_binding_route_snapshot(&target.actor_ref, reference))
            .transpose()?;
        if binding.adapter_ref == UPSTREAM_ADAPTER_REF {
            return Ok(Some(MessagePreflightReadiness {
                binding_ref: binding.actor_integration_ref.clone(),
                process: None,
                preference,
                route_snapshot,
            }));
        }
        Ok(Some(MessagePreflightReadiness {
            binding_ref: binding.actor_integration_ref.clone(),
            preference,
            route_snapshot,
            process: Some(match checked.remove(selected_binding) {
                Some(checked) => checked,
                None => {
                    let route = binding.process_adapter_route()?;
                    (route.fingerprint(), readiness(&route))
                }
            }),
        }))
    }

    fn handle_send_message(
        &self,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
        queue: &Rc<RefCell<VecDeque<String>>>,
        model_output: &AdapterModelOutputMailbox,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<AdapterHostCallOutcome> {
        let input: SendMessageInput = parse_input("send-message", &call.input)?;
        if input.text.trim().is_empty() {
            return invalid("message text must not be blank");
        }
        let (working_ref, caller_execution_group_ref) = self.message_working_scope(caller)?;
        let conversation_ref = working_ref.as_str();
        let preflight_readiness = self.message_preflight_readiness(&input, call, caller);
        let invocation_ref = {
            let mut state = self.lock()?;
            validate_message_call(&state, call, input.runtime_ref.as_deref())?;
            let owner = state
                .host_local_invocation_owners
                .get(&caller.invocation_ref)
                .cloned();
            let existing = state.admission.invocation_records().find(|record| matches!(
                &record.origin, InvocationOrigin::HostCall { caller_invocation_ref, call_id }
                if caller_invocation_ref == &call.invocation_ref && call_id == &call.call_id
            )).map(|record| record.invocation_ref.clone());
            if let Some(existing) = existing {
                recover_message_call(&mut state, call, owner, existing)?
            } else {
                let preflight_readiness = preflight_readiness?;
                let graph = upstream_graph(&state, owner.as_deref())?;
                let catalog = local_catalog(&graph, &caller.target_actor_ref)?;
                let target = catalog
                    .known_actor_by_ref(&input.to)
                    .or_else(|| catalog.known_actor_by_handle(&input.to))
                    .ok_or_else(|| invalid_error(format!("unknown participant {}", input.to)))?;
                if target.actor_ref == caller.target_actor_ref {
                    return invalid("send_message addresses another coworker, not the caller");
                }
                let allowed = action_actor_allowlist(
                    &graph,
                    &caller.target_actor_ref,
                    caller.actor_integration_ref.as_deref(),
                )?;
                if allowed
                    .as_ref()
                    .is_some_and(|refs| !refs.contains(&target.actor_ref))
                {
                    return invalid("coworker request is not authorized");
                }

                let Some(actor) = catalog.actor_by_ref(&target.actor_ref) else {
                    if input.binding_ref.is_some() {
                        return invalid("explicit message binding is unavailable");
                    }
                    if graph
                        .actor_integrations
                        .iter()
                        .any(|binding| binding.actor_ref == target.actor_ref)
                    {
                        return invalid(format!(
                            "{} has no locally supported delivery route",
                            input.to
                        ));
                    }
                    let ingress = ConversationIngress {
                        owner_context_ref: None,
                        file_refs: Vec::new(),
                        integration_ref: INTRINSIC_CWR_INTEGRATION_REF.into(),
                        provider_event_ref: format!("{}#{}", call.invocation_ref, call.call_id),
                        author_actor_ref: caller.target_actor_ref.clone(),
                        conversation: ConversationIngressConversation::Continue {
                            conversation_ref: conversation_ref.into(),
                        },
                        addressing: ConversationIngressAddressing::Addressed {
                            actor_refs: vec![target.actor_ref.clone()],
                        },
                        text: input.text,
                    };
                    state.admission.admit(&graph, &ingress, None)?;
                    return Ok(AdapterHostCallOutcome::Returned(json!({
                        "outcome":"awaiting_host_reply", "to":target.actor_ref,
                        "text":"Message recorded for host delivery; no reply has been received."})));
                };
                let selected_binding = input
                    .binding_ref
                    .as_deref()
                    .or_else(|| {
                        preflight_readiness
                            .as_ref()
                            .map(|preflight| preflight.binding_ref.as_str())
                    })
                    .unwrap_or(&actor.actor_integration_ref);
                let binding = graph
                    .actor_integrations
                    .iter()
                    .find(|b| {
                        b.actor_ref == target.actor_ref
                            && b.actor_integration_ref == selected_binding
                    })
                    .ok_or_else(|| invalid_error("message route is unavailable"))?;
                binding.require_available()?;
                if let Some(preflight) = &preflight_readiness {
                    let preference = graph
                        .actors
                        .iter()
                        .find(|actor| actor.actor_ref == target.actor_ref)
                        .map(|actor| actor.message_binding_preference.as_slice())
                        .unwrap_or_default();
                    validate_message_preflight(&graph, binding, preflight, preference)?;
                }
                if binding.adapter_ref == crate::adapter::PERSON_ADAPTER_REF {
                    if state
                        .admission
                        .conversation_header(conversation_ref)
                        .is_some_and(|c| c.parent_conversation_ref.is_some())
                    {
                        return invalid("person response requests require a shared root conversation; return the question to the requesting conversation");
                    }
                    let ingress = ConversationIngress {
                        owner_context_ref: None,
                        file_refs: Vec::new(),
                        integration_ref: INTRINSIC_CWR_INTEGRATION_REF.into(),
                        provider_event_ref: format!("{}#{}", call.invocation_ref, call.call_id),
                        author_actor_ref: caller.target_actor_ref.clone(),
                        conversation: ConversationIngressConversation::Continue {
                            conversation_ref: conversation_ref.into(),
                        },
                        addressing: ConversationIngressAddressing::Addressed {
                            actor_refs: vec![target.actor_ref.clone()],
                        },
                        text: input.text,
                    };
                    let admitted = state.admission.admit(
                        &graph,
                        &ingress,
                        Some(&binding.actor_integration_ref),
                    )?;
                    let reference = match admitted {
                        ConversationAdmissionOutcome::Started(turn) => turn.record.invocation_ref,
                        ConversationAdmissionOutcome::Existing(record) => record.invocation_ref,
                    };
                    return Ok(AdapterHostCallOutcome::Returned(
                        json!({"outcome":"awaiting_host_reply", "invocation_ref":reference, "to":target.actor_ref}),
                    ));
                }
                require_action_depth(&state.admission, caller)?;
                let invocation_ref = new_invocation_ref(state.admission.runtime_root_ref());
                let mut invocation = binding.project_adapter_invocation(
                    invocation_ref.clone(), RESPOND_TO_MESSAGE_ACTION_NAME, json!({}),
                    json!({"type":"object", "required":["text"], "properties":{"text":{"type":"string"}}}),
                    state.admission.thread_observation(conversation_ref)?,
                );
                invocation.source_actor_ref = Some(caller.target_actor_ref.clone());
                let request = StartActionInvocation {
                    invocation_ref: invocation_ref.clone(),
                    origin: InvocationOrigin::HostCall {
                        caller_invocation_ref: call.invocation_ref.clone(),
                        call_id: call.call_id.clone(),
                    },
                    requester_actor_ref: caller.target_actor_ref.clone(),
                    target_actor_ref: target.actor_ref.clone(),
                    conversation_ref: Some(conversation_ref.into()),
                    actor_integration_ref: Some(binding.actor_integration_ref.clone()),
                    action_name: RESPOND_TO_MESSAGE_ACTION_NAME.into(),
                    input: json!({}),
                    parent_invocation_ref: Some(caller.invocation_ref.clone()),
                    admitted_at_unix_ms: current_unix_ms(),
                };
                state.admission.start_child_from_host_action_call(
                    call,
                    ConversationHostActionCall {
                        conversation_ref,
                        caller_invocation_ref: &call.invocation_ref,
                        host_call_id: &call.call_id,
                        nested_invocation_ref: &invocation_ref,
                        caller_actor_ref: &caller.target_actor_ref,
                        target_actor_ref: &target.actor_ref,
                        activation: &target.handle,
                        input: json!({}),
                        text: input.text,
                    },
                    request,
                    invocation,
                )?;
                if let Some(owner) = owner {
                    state
                        .host_local_invocation_owners
                        .insert(invocation_ref.clone(), owner);
                }
                invocation_ref
            }
        };
        let response = self.execute_until_terminal(
            &caller_execution_group_ref,
            &invocation_ref,
            queue,
            model_output,
            observer,
            None,
        )?;
        Ok(AdapterHostCallOutcome::Returned(serde_json::to_value(
            response,
        )?))
    }
}

impl LocalRuntime {
    fn message_working_scope(&self, caller: &ActionInvocationRecord) -> CwrResult<(String, String)> {
        let state = self.lock()?;
        Ok((state.admission.working_conversation_ref(caller)?.to_owned(), execution_group_ref(&state.admission, caller).ok_or_else(|| invalid_error("caller invocation has no execution group"))?.to_owned()))
    }
}

fn recover_message_call(state: &mut LocalRuntimeState, call: &AdapterCall, owner: Option<String>, existing: String) -> CwrResult<String> {
                let record = state
                    .admission
                    .invocation_record(&existing)
                    .ok_or_else(|| invalid_error("message invocation is unavailable"))?;
                if record.is_runnable() {
                    require_current_request_authority(state, record)?;
                }
                ensure_host_call(&mut state.admission, call)?;
                if let Some(owner) = owner {
                    state
                        .host_local_invocation_owners
                        .entry(existing.clone())
                        .or_insert(owner);
                }
                Ok(existing)
}

fn validate_message_call(state: &LocalRuntimeState, call: &AdapterCall, runtime_ref: Option<&str>) -> CwrResult<()> {
            if runtime_ref
                .is_some_and(|reference| reference != state.graph.runtime_ref)
            {
                return invalid("cross-runtime collaboration is not supported");
            }
            require_no_stop_fence(state, &call.invocation_ref)?;
            require_active_caller(&state.admission, call)?;
            if state.shutting_down {
                return invalid("local runtime is shutting down");
            }
    Ok(())
}
