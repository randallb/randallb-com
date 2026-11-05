include!("archive_control.rs");

impl LocalRuntime {
    fn prepare_conversation_rename_as(
        &self,
        conversation_ref: Option<&str>,
        caller: &str,
        title: &str,
    ) -> CwrResult<(IntrinsicCwrResponse, ConversationRenameMutation)> {
        let conversation_ref =
            conversation_ref.ok_or_else(|| invalid_error("CWR invocation has no conversation"))?;
        let rename =
            self.lock()?
                .admission
                .prepare_conversation_rename(conversation_ref, caller, title)?;
        let response = intrinsic_cwr_conversation_renamed(
            rename.conversation_ref.clone(),
            rename.title.clone(),
        );
        Ok((response, rename))
    }

    pub fn rename_conversation_as(
        &self,
        conversation_ref: Option<&str>,
        caller: &str,
        title: &str,
    ) -> CwrResult<IntrinsicCwrResponse> {
        let conversation_ref =
            conversation_ref.ok_or_else(|| invalid_error("CWR invocation has no conversation"))?;
        let conversation =
            self.lock()?
                .admission
                .rename_conversation(conversation_ref, caller, title)?;
        Ok(intrinsic_cwr_conversation_renamed(
            conversation.conversation_ref,
            conversation
                .title
                .expect("renamed conversation has a title"),
        ))
    }

    pub fn invite_coworker_as(
        &self,
        conversation_ref: &str,
        caller: &str,
        to: &str,
    ) -> CwrResult<Value> {
        let mut state = self.lock()?;
        if state.shutting_down {
            return invalid("local runtime is shutting down");
        }
        let catalog = local_catalog(&state.graph, caller)?;
        let actor = catalog
            .known_actor_by_ref(to)
            .or_else(|| catalog.known_actor_by_handle(to))
            .ok_or_else(|| invalid_error(format!("unknown coworker actor {to}")))?;
        let graph = state.graph.clone();
        let added =
            state
                .admission
                .invite_actor(&graph, conversation_ref, caller, &actor.actor_ref)?;
        Ok(json!({"actor_ref":actor.actor_ref, "added":added, "participation":"participant"}))
    }

    fn conversation_intrinsic_response(
        &self,
        record: &ActionInvocationRecord,
        command: &IntrinsicCwrCommand,
    ) -> CwrResult<Option<IntrinsicCwrResponse>> {
        let (caller_record, owner) = {
            let state = self.lock()?;
            let caller_record = record
                .parent_invocation_ref
                .as_deref()
                .and_then(|r| state.admission.invocation_record(r))
                .cloned();
            let owner = state
                .host_local_invocation_owners
                .get(&record.invocation_ref)
                .cloned();
            (caller_record, owner)
        };
        self.discovery_as_in_host_scope(
            record.conversation_ref.as_deref(),
            &record.requester_actor_ref,
            caller_record
                .as_ref()
                .and_then(|caller| caller.actor_integration_ref.as_deref()),
            caller_record.as_ref(),
            owner.as_deref(),
            command,
        )
    }

    pub fn discovery_as(
        &self,
        conversation_ref: Option<&str>,
        caller: &str,
        caller_integration: Option<&str>,
        command: &IntrinsicCwrCommand,
    ) -> CwrResult<Option<IntrinsicCwrResponse>> {
        self.discovery_as_in_host_scope(
            conversation_ref,
            caller,
            caller_integration,
            None,
            None,
            command,
        )
    }

    fn discovery_as_in_host_scope(
        &self,
        conversation_ref: Option<&str>,
        caller: &str,
        caller_integration: Option<&str>,
        caller_invocation: Option<&ActionInvocationRecord>,
        owner: Option<&str>,
        command: &IntrinsicCwrCommand,
    ) -> CwrResult<Option<IntrinsicCwrResponse>> {
        if !matches!(
            command,
            IntrinsicCwrCommand::ListCoworkers
                | IntrinsicCwrCommand::ListActions
                | IntrinsicCwrCommand::ShowHelp
        ) {
            return Ok(None);
        }
        let (graph, participants, host_local_actor_refs, allowlist, runtime_ref, retained) = {
            let state = self.lock()?;
            let mut graph = upstream_graph(&state, owner)?;
            let participants = if let Some(conversation_ref) = conversation_ref {
                state
                    .admission
                    .conversation_header(conversation_ref)
                    .ok_or_else(|| invalid_error("discovery conversation is unavailable"))?
                    .participant_actor_refs
                    .clone()
            } else {
                if !graph.actors.iter().any(|actor| actor.actor_ref == caller) {
                    return invalid("discovery caller is unknown");
                }
                vec![caller.to_owned()]
            };
            if !participants.iter().any(|r| r == caller) {
                return invalid("discovery caller is not a participant");
            }
            let identities = graph
                .actors
                .iter()
                .map(|actor| {
                    Ok((
                        actor.actor_ref.clone(),
                        runtime_actor_identity(&graph, actor)?,
                    ))
                })
                .collect::<CwrResult<BTreeMap<_, _>>>()?;
            scope_discovery_graph(
                &state,
                conversation_ref,
                caller,
                caller_invocation,
                &mut graph,
            )?;
            let host_local_actor_refs = state
                .upstream
                .visible(owner)?
                .into_iter()
                .map(|route| route.coworker.actor_ref)
                .collect::<Vec<_>>();
            let allowlist = action_actor_allowlist(&state.graph, caller, caller_integration)?;
            let retained = participants
                .iter()
                .filter(|actor_ref| {
                    !graph
                        .actors
                        .iter()
                        .any(|actor| &actor.actor_ref == *actor_ref)
                })
                .map(|actor_ref| {
                    let (handle, name) = if let Some(identity) = identities.get(actor_ref) {
                        (identity.handle.clone(), identity.name.clone())
                    } else if let Some(actor) = state
                        .catalog
                        .retained_actors
                        .iter()
                        .find(|actor| &actor.actor_ref == actor_ref)
                    {
                        // Disabled local pack members keep their authored identity;
                        // resource refs do not encode conversational handles.
                        (actor.handle.clone(), actor.name.clone())
                    } else if let Some(actor) = state
                        .admission
                        .retained_upstream_actor(conversation_ref.unwrap_or_default(), actor_ref)?
                    {
                        (actor.handle, actor.name)
                    } else {
                        (actor_handle(actor_ref)?, "Host participant".into())
                    };
                    Ok((actor_ref.clone(), handle, name))
                })
                .collect::<CwrResult<Vec<_>>>()?;
            (
                graph,
                participants,
                host_local_actor_refs,
                allowlist,
                state.graph.runtime_ref.clone(),
                retained,
            )
        };
        let mut routable_actor_refs = graph
            .actors
            .iter()
            .map(|actor| actor.actor_ref.clone())
            .collect::<Vec<_>>();
        routable_actor_refs.extend(host_local_actor_refs.iter().cloned());
        let mut routes =
            action_routes_for_graph(&graph, &routable_actor_refs, caller, allowlist.as_ref())?;
        let required_bindings = if matches!(command, IntrinsicCwrCommand::ListCoworkers) {
            graph
                .actor_integrations
                .iter()
                .map(|binding| binding.actor_integration_ref.clone())
                .collect()
        } else {
            routes
                .values()
                .map(|route| route.actor_integration_ref.clone())
                .collect()
        };
        let readiness_by_binding = adapter_readiness_by_binding(&graph, &required_bindings);
        routes.retain(|_, route| route_readiness(route, &readiness_by_binding).is_ready());
        let actions = routes.keys().cloned().collect::<Vec<_>>();
        if matches!(command, IntrinsicCwrCommand::ListActions) {
            return Ok(Some(IntrinsicCwrResponse::Actions {
                targets: routes
                    .iter()
                    .map(|(name, route)| (name.clone(), route_action_target(&runtime_ref, route)))
                    .collect(),
                actions,
                descriptions: action_descriptions(&routes),
                input_schemas: routes
                    .iter()
                    .map(|(name, route)| (name.clone(), route.input_schema.clone()))
                    .collect(),
            }));
        }
        if matches!(command, IntrinsicCwrCommand::ShowHelp) {
            return Ok(Some(IntrinsicCwrResponse::Help { actions }));
        }
        let mut coworkers = Vec::new();
        for actor in &graph.actors {
            let host_local = host_local_actor_refs.contains(&actor.actor_ref);
            let participant = participants.contains(&actor.actor_ref)
                || actor.actor_ref == INTRINSIC_CWR_ACTOR_REF;
            let identity = runtime_actor_identity(&graph, actor)?;
            coworkers.push(IntrinsicCwrCatalogCoworker {
                handle: identity.handle,
                name: identity.name,
                details: coworker_details(
                    &runtime_ref,
                    &graph,
                    actor,
                    participant,
                    host_local,
                    &readiness_by_binding,
                ),
                actions: routes
                    .iter()
                    .filter(|(_, r)| r.actor_ref == actor.actor_ref)
                    .map(|(a, _)| a.clone())
                    .collect(),
            });
        }
        for (actor_ref, handle, name) in retained {
            coworkers.push(IntrinsicCwrCatalogCoworker {
                details: BTreeMap::from([
                    ("coworker_ref".into(), json!(actor_ref)),
                    ("available".into(), json!(false)),
                ]),
                handle,
                name: format!("{name} (unavailable)"),
                actions: Vec::new(),
            });
        }
        Ok(Some(IntrinsicCwrResponse::Coworkers { coworkers }))
    }
}

fn coworker_details(
    runtime_ref: &str,
    graph: &ResolvedCwrGraph,
    actor: &ResolvedCwrActor,
    participant: bool,
    host_local: bool,
    readiness_by_binding: &BTreeMap<String, AdapterReadiness>,
) -> BTreeMap<String, Value> {
    let bindings = graph.actor_integrations.iter().filter(|binding| binding.actor_ref == actor.actor_ref)
        .map(|binding| {
            let readiness = readiness_by_binding[&binding.actor_integration_ref].clone();
            json!({
                "binding_ref": binding.actor_integration_ref,
                "default": actor.default_actor_integration_ref.as_deref() == Some(binding.actor_integration_ref.as_str()),
                "available": binding.is_available() && readiness.is_ready(),
                "readiness": readiness,
                "accepts_messages": binding.message_mode == crate::v2026_09::graph::ActorIntegrationMessageMode::Conversational,
            })
        }).collect::<Vec<_>>();
    let description = graph
        .resources
        .iter()
        .find(|resource| resource.reference == actor.actor_ref)
        .and_then(|resource| resource.document.get("description"))
        .cloned()
        .unwrap_or(json!(""));
    BTreeMap::from([
        ("runtime_ref".into(), json!(runtime_ref)),
        ("coworker_ref".into(), json!(actor.actor_ref)),
        ("role_refs".into(), json!(actor.role_refs)),
        ("description".into(), description),
        ("bindings".into(), json!(bindings)),
        ("participant".into(), json!(participant)),
        ("host_local".into(), json!(host_local)),
        (
            "delivery".into(),
            json!(if bindings.is_empty() {
                "host_required"
            } else {
                "configured"
            }),
        ),
    ])
}

include!("discovery_readiness.rs");


const LIST_COWORKERS_HOST_OPERATION: &str = "cwr.list_coworkers";
const LIST_ACTIONS_HOST_OPERATION: &str = "cwr.list_actions";

fn discovery_operations() -> [AdapterHostOperation; 2] {
    [
        AdapterHostOperation {
            name: LIST_COWORKERS_HOST_OPERATION.into(),
            description: "Discover coworkers in this runtime, their roles, bindings, and availability.".into(),
            input_schema: json!({"type":"object", "additionalProperties":false}),
            output_schema: json!({"type":"object"}),
        },
        AdapterHostOperation {
            name: LIST_ACTIONS_HOST_OPERATION.into(),
            description: "Discover action schemas and exact targets. Optionally select a coworker and one named binding.".into(),
            input_schema: json!({"type":"object", "additionalProperties":false,"properties": {
                "coworker_ref":{"type":"string", "description":"An exact coworker reference."},
                "coworker_handle":{"type":"string", "description":"An unambiguous coworker @handle."},
                "binding_ref":{"type":"string"}
            }}),
            output_schema: json!({"type":"object"}),
        },
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionDiscoveryInput {
    coworker_ref: Option<String>,
    coworker_handle: Option<String>,
    binding_ref: Option<String>,
}

fn resolve_discovery_coworker_ref(
    graph: &ResolvedCwrGraph,
    requested_ref: Option<&str>,
    requested_handle: Option<&str>,
) -> CwrResult<Option<String>> {
    if requested_ref.is_some() && requested_handle.is_some() {
        return invalid("list actions accepts either coworker_ref or coworker_handle, not both");
    }
    if let Some(requested_ref) = requested_ref {
        if graph
            .actors
            .iter()
            .any(|actor| actor.actor_ref == requested_ref)
        {
            return Ok(Some(requested_ref.to_owned()));
        }
        return invalid(format!(
            "unknown discovery coworker reference {requested_ref}; call list--coworkers and use its coworker_ref"
        ));
    }
    let Some(requested_handle) = requested_handle else {
        return Ok(None);
    };
    let mut matches = Vec::new();
    for actor in &graph.actors {
        if runtime_actor_identity(graph, actor)?.handle == requested_handle {
            matches.push(actor.actor_ref.clone());
        }
    }
    match matches.as_slice() {
        [actor_ref] => Ok(Some(actor_ref.clone())),
        [] => invalid(format!(
            "unknown discovery coworker handle {requested_handle}; call list--coworkers and use its handle"
        )),
        actor_refs => invalid(format!(
            "discovery coworker handle {requested_handle} is ambiguous ({actor_refs:?}); call list--coworkers and use an exact coworker_ref"
        )),
    }
}

impl LocalRuntime {
    fn handle_discovery_call(
        &self,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
    ) -> CwrResult<AdapterHostCallOutcome> {
        let (conversation, owner) = {
            let state = self.lock()?;
            require_no_stop_fence(&state, &caller.invocation_ref)?;
            require_active_caller(&state.admission, call)?;
            (
                state.admission.working_conversation_ref(caller)?.to_owned(),
                state
                    .host_local_invocation_owners
                    .get(&caller.invocation_ref)
                    .cloned(),
            )
        };
        if call.operation == LIST_COWORKERS_HOST_OPERATION {
            if call.input != json!({}) {
                return invalid("list coworkers takes an empty object");
            }
            let response = self.discovery_as_in_host_scope(
                Some(&conversation),
                &caller.target_actor_ref,
                caller.actor_integration_ref.as_deref(),
                Some(caller),
                owner.as_deref(),
                &IntrinsicCwrCommand::ListCoworkers,
            )?;
            return Ok(AdapterHostCallOutcome::Returned(serde_json::to_value(
                response,
            )?));
        }
        let input: ActionDiscoveryInput = parse_input("list-actions", &call.input)?;
        let (graph, allowlist, runtime_ref, can_reconfigure, gateway_host_refs, catalog_caller) = {
            let state = self.lock()?;
            let mut graph = upstream_graph(&state, owner.as_deref())?;
            scope_discovery_graph(
                &state,
                Some(&conversation),
                &caller.target_actor_ref,
                Some(caller),
                &mut graph,
            )?;
            let gateway = gateway_route_for_record(&state.graph, caller).is_some();
            let allowlist = action_actor_allowlist(
                &state.graph,
                &caller.target_actor_ref,
                caller.actor_integration_ref.as_deref(),
            )?;
            let host_refs = if gateway {
                Some(
                    state
                        .upstream
                        .visible(owner.as_deref())?
                        .into_iter()
                        .map(|route| route.coworker.actor_ref)
                        .collect::<BTreeSet<_>>(),
                )
            } else {
                None
            };
            let can_reconfigure =
                require_reconfiguration_authority(&state, &caller.target_actor_ref).is_ok();
            (
                graph,
                allowlist,
                state.graph.runtime_ref.clone(),
                can_reconfigure && !gateway,
                host_refs,
                if gateway {
                    caller.requester_actor_ref.clone()
                } else {
                    caller.target_actor_ref.clone()
                },
            )
        };
        let coworker_ref = resolve_discovery_coworker_ref(
            &graph,
            input.coworker_ref.as_deref(),
            input.coworker_handle.as_deref(),
        )?;
        let catalog = coworker_ref
            .is_none()
            .then(|| local_catalog(&graph, &catalog_caller))
            .transpose()?;
        let mut actions = Vec::new();
        let candidate_routes = graph
            .routes
            .iter()
            .filter(|route| {
                if coworker_ref
                    .as_ref()
                    .is_some_and(|reference| &route.actor_ref != reference)
                    || input
                        .binding_ref
                        .as_ref()
                        .is_some_and(|reference| &route.actor_integration_ref != reference)
                    || allowlist
                        .as_ref()
                        .is_some_and(|refs| !refs.contains(&route.actor_ref))
                    || gateway_host_refs
                        .as_ref()
                        .is_some_and(|refs| !refs.contains(&route.actor_ref))
                    || !route.is_available()
                {
                    return false;
                }
                if route.actor_ref == INTRINSIC_CWR_ACTOR_REF
                    && matches!(
                        route.action_name.as_str(),
                        "reload--graph"
                            | "read--experimental_features"
                            | "set--experimental_features"
                    )
                    && !can_reconfigure
                {
                    return false;
                }
                true
            })
            .collect::<Vec<_>>();
        let required_bindings = candidate_routes
            .iter()
            .map(|route| route.actor_integration_ref.clone())
            .collect();
        let readiness_by_binding = adapter_readiness_by_binding(&graph, &required_bindings);
        for route in candidate_routes {
            let actor = match catalog.as_ref() {
                Some(catalog) => catalog.known_actor_by_ref(&route.actor_ref).cloned(),
                None => graph
                    .actors
                    .iter()
                    .find(|actor| actor.actor_ref == route.actor_ref)
                    .map(|actor| runtime_actor_identity(&graph, actor))
                    .transpose()?,
            };
            let Some(actor) = actor else {
                continue;
            };
            let activation = format!("{}--{}", actor.handle, route.action_name);
            let readiness = route_readiness(route, &readiness_by_binding);
            actions.push(json!({
                "target": route_action_target(&runtime_ref, route), "activation": activation,
                "is_default_binding": graph.actors.iter().find(|actor| actor.actor_ref == route.actor_ref)
                    .and_then(|actor| actor.default_actor_integration_ref.as_deref()) == Some(route.actor_integration_ref.as_str()),
                "model_tool": format!("cwp.{}--{}", actor.handle.trim_start_matches('@'), route.action_name),
                "description": route.action_description, "input_schema": route.input_schema,
                "available": readiness.is_ready(), "readiness": readiness,
            }));
        }
        Ok(AdapterHostCallOutcome::Returned(
            json!({"actions": actions}),
        ))
    }
}
