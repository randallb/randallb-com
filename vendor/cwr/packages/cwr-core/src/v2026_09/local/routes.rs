#[cfg(test)]
fn visible_action_routes(
    state: &LocalRuntimeState,
    conversation_ref: &str,
    caller_actor_ref: &str,
    allowed_action_actor_refs: Option<&BTreeSet<String>>,
) -> CwrResult<BTreeMap<String, ResolvedCwrActionRoute>> {
    visible_action_routes_for_integration(
        state,
        conversation_ref,
        caller_actor_ref,
        allowed_action_actor_refs,
        None,
    )
}

fn visible_action_routes_for_integration(
    state: &LocalRuntimeState,
    conversation_ref: &str,
    caller_actor_ref: &str,
    allowed_action_actor_refs: Option<&BTreeSet<String>>,
    selected_integration: Option<(&str, &str)>,
) -> CwrResult<BTreeMap<String, ResolvedCwrActionRoute>> {
    visible_action_routes_in_upstream_scope(
        state,
        conversation_ref,
        caller_actor_ref,
        allowed_action_actor_refs,
        selected_integration,
        None,
    )
}

fn visible_action_routes_in_upstream_scope(
    state: &LocalRuntimeState,
    conversation_ref: &str,
    caller_actor_ref: &str,
    allowed_action_actor_refs: Option<&BTreeSet<String>>,
    selected_integration: Option<(&str, &str)>,
    owner: Option<&str>,
) -> CwrResult<BTreeMap<String, ResolvedCwrActionRoute>> {
    let participants = state
        .admission
        .conversation_header(conversation_ref)
        .ok_or_else(|| invalid_error(format!("unknown conversation ref {conversation_ref}")))?
        .participant_actor_refs
        .clone();
    let graph = upstream_graph(state, owner)?;
    let mut visible_actor_refs = participants;
    visible_actor_refs.extend(
        state
            .upstream
            .visible(owner)?
            .into_iter()
            .map(|route| route.coworker.actor_ref),
    );
    let mut routes = action_routes_for_graph_selected(
        &graph,
        &visible_actor_refs,
        caller_actor_ref,
        allowed_action_actor_refs,
        selected_integration,
    )?;
    if !state.allow_reconfiguration {
        routes.retain(|_, route| route.actor_ref != INTRINSIC_CWR_ACTOR_REF || route.action_name != "reload--graph");
    }
    Ok(routes)
}

fn action_routes_for_graph(
    graph: &ResolvedCwrGraph,
    _participants: &[String],
    caller_actor_ref: &str,
    allowed_action_actor_refs: Option<&BTreeSet<String>>,
) -> CwrResult<BTreeMap<String, ResolvedCwrActionRoute>> {
    action_routes_for_graph_selected(
        graph,
        _participants,
        caller_actor_ref,
        allowed_action_actor_refs,
        None,
    )
}

fn action_routes_for_graph_selected(
    graph: &ResolvedCwrGraph,
    _participants: &[String],
    caller_actor_ref: &str,
    allowed_action_actor_refs: Option<&BTreeSet<String>>,
    selected_integration: Option<(&str, &str)>,
) -> CwrResult<BTreeMap<String, ResolvedCwrActionRoute>> {
    let catalog = local_catalog(graph, caller_actor_ref)?;
    let mut routes = BTreeMap::new();
    for route in &graph.routes {
        if selected_integration.is_some_and(|(actor_ref, integration_ref)| {
            route.actor_ref != actor_ref || route.actor_integration_ref != integration_ref
        }) || route.actor_ref == caller_actor_ref
            || allowed_action_actor_refs.is_some_and(|allowed| !allowed.contains(&route.actor_ref))
            || !route.is_available()
            || (route.adapter_ref != UPSTREAM_ADAPTER_REF
                && !route.process_adapter_route()?.is_supported_by_local_host())
        {
            continue;
        }
        let Some(actor) = catalog.actor_by_ref(&route.actor_ref) else {
            continue;
        };
        if selected_integration.is_none() && actor.actor_integration_ref != route.actor_integration_ref {
            continue;
        }
        if route.actor_ref == INTRINSIC_CWR_ACTOR_REF && route.action_name == "reload--graph"
            && !graph.actors.iter().any(|actor| actor.actor_ref == caller_actor_ref && actor.authority.iter().any(|grant| grant == "cwr:configure")) {
            continue;
        }
        let activation = format!("{}--{}", actor.handle, route.action_name);
        if routes.insert(activation.clone(), route.clone()).is_some() {
            return invalid(format!(
                "visible action activation {activation} resolves more than once"
            ));
        }
    }
    Ok(routes)
}

fn action_actor_allowlist(
    graph: &ResolvedCwrGraph,
    actor_ref: &str,
    actor_integration_ref: Option<&str>,
) -> CwrResult<Option<BTreeSet<String>>> {
    let Some(actor_integration_ref) = actor_integration_ref else {
        return Ok(None);
    };
    let params = graph.actor_integrations.iter()
        .find(|binding| binding.actor_ref == actor_ref && binding.actor_integration_ref == actor_integration_ref)
        .map(|binding| &binding.actor_integration_params)
        .or_else(|| graph.gateways.iter().flat_map(|gateway| [
            Some(&gateway.route), gateway.ingress_route.as_ref(), gateway.delivery_route.as_ref()
        ]).flatten().find(|route| route.actor_ref == actor_ref && route.actor_integration_ref == actor_integration_ref)
            .map(|route| &route.actor_integration_params))
        .ok_or_else(|| invalid_error("invocation actor integration is not in the graph"))?;
    let Some(value) = params.get("cwr_action_actor_refs") else {
        return Ok(None);
    };
    let refs = value
        .as_array()
        .ok_or_else(|| invalid_error("cwr_action_actor_refs must be an array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid_error("cwr_action_actor_refs must contain strings"))
        })
        .collect::<CwrResult<BTreeSet<_>>>()?;
    Ok(Some(refs))
}

fn require_current_request_authority(
    state: &LocalRuntimeState,
    record: &ActionInvocationRecord,
) -> CwrResult<()> {
    state.admission.check_context_execution(&state.graph, record)?;

    let Some(parent_ref) = record.parent_invocation_ref.as_deref() else {
        return Ok(());
    };
    let parent = state.admission.invocation_record(parent_ref)
        .ok_or_else(|| invalid_error("requesting invocation is unavailable"))?;
    let allowed = action_actor_allowlist(&state.graph, &parent.target_actor_ref,
        parent.actor_integration_ref.as_deref())?;
    if allowed.as_ref().is_some_and(|refs| !refs.contains(&record.target_actor_ref)) {
        return invalid("coworker request is no longer authorized");
    }
    if let Some(route) = retained_upstream_route(state, &record.invocation_ref) {
        if let Some(resource) = &route.gateway_resource_ref {
            let owner = state.host_local_invocation_owners.get(parent_ref)
                .ok_or_else(|| invalid_error("gateway caller connection is unavailable"))?;
            let handler = state.gateway_handlers.get(&(owner.clone(), resource.clone()))
                .filter(|handler| handler.route == route)
                .ok_or_else(|| invalid_error("gateway handler registration is unavailable"))?;
            let gateway = state.graph.gateways.iter().find(|gateway| &gateway.resource_ref == resource)
                .ok_or_else(|| invalid_error("gateway resource is unavailable"))?;
            if gateway.approval_fingerprint()? != handler.fingerprint {
                return invalid("gateway definition changed after registration");
            }
        }
    }
    if gateway_route_for_record(&state.graph, parent).is_some() {
        let owner = state.host_local_invocation_owners.get(parent_ref).map(String::as_str);
        if state.upstream.visible(owner)?.iter().any(|route| route.coworker.actor_ref == record.target_actor_ref) {
            return Ok(());
        }
        return invalid("gateway host action is no longer available");
    }
    Ok(())
}

fn require_reconfiguration_authority(state: &LocalRuntimeState, requester: &str) -> CwrResult<()> {
    if !state.allow_reconfiguration || !state.graph.actors.iter().any(|actor|
        actor.actor_ref == requester && actor.authority.iter().any(|grant| grant == "cwr:configure")) {
        return invalid("workspace configuration capability is required");
    }
    Ok(())
}

fn action_descriptions(
    routes: &BTreeMap<String, ResolvedCwrActionRoute>,
) -> BTreeMap<String, String> {
    routes
        .iter()
        .map(|(activation, route)| (activation.clone(), route.action_description.clone()))
        .collect()
}

fn invoke_action_operation(
    routes: &BTreeMap<String, ResolvedCwrActionRoute>,
) -> AdapterHostOperation {
    let _ = routes;
    AdapterHostOperation {
        name: INVOKE_ACTION_HOST_OPERATION.into(),
        description: "Invoke an exact target returned by action discovery. Current permissions and the action contract are checked before work starts.".into(),
        input_schema: invoke_action_input_schema(),
        output_schema: invoke_action_output_schema(),
    }
}

fn adapter_terminal(terminal: &AdapterTerminal) -> CwrResult<InvocationTerminal> {
    let status = ActionResponseStatus::new(terminal.status)
        .map_err(|error| invalid_error(error.to_string()))?;
    if status.is_stopped() {
        return invalid("an adapter result cannot create host status 499");
    }
    if status.is_progress() {
        return invalid(format!(
            "adapter terminal status must not be 1xx, got {}",
            status.as_u16()
        ));
    }
    if status.is_completed() {
        return Ok(InvocationTerminal {
            status,
            summary: terminal.summary.clone(),
            result: Some(
                terminal
                    .result
                    .clone()
                    .ok_or_else(|| invalid_error("adapter completed without a result"))?,
            ),
            error: None,
        });
    }
    let detail = terminal
        .error
        .as_ref()
        .map(|error| {
            let mut detail = ActionResponseDetail::new(error.code.clone());
            if let Some(message) = &error.message {
                detail = detail.with_message(message.clone());
            }
            if let Some(details) = &error.details {
                detail = detail.with_details(details.clone());
            }
            detail
        })
        .ok_or_else(|| invalid_error("adapter error result has no error detail"))?;
    debug_assert!(status.is_blocked() || status.is_failed());
    Ok(InvocationTerminal {
        status,
        summary: terminal.summary.clone(),
        result: None,
        error: Some(detail),
    })
}

#[cfg(test)]
fn exact_action_target(state: &LocalRuntimeState, route: &ResolvedCwrActionRoute) -> ExactActionTarget {
    route_action_target(&state.graph.runtime_ref, route)
}

fn resolve_exact_action(
    state: &LocalRuntimeState,
    caller: &ActionInvocationRecord,
    target: &ExactActionTarget,
    owner: Option<&str>,
) -> CwrResult<ResolvedCwrActionRoute> {
    if target.runtime_ref != state.graph.runtime_ref {
        return invalid(format!("cross-runtime collaboration is not supported: {}", target.runtime_ref));
    }
    let allowed = action_actor_allowlist(&state.graph, &caller.target_actor_ref, caller.actor_integration_ref.as_deref())?;
    if allowed.as_ref().is_some_and(|refs| !refs.contains(&target.coworker_ref)) {
        return invalid("action target is not authorized");
    }
    if gateway_route_for_record(&state.graph, caller).is_some()
        && !state.upstream.visible(owner)?.iter().any(|route| route.coworker.actor_ref == target.coworker_ref) {
        return invalid("gateway may only invoke an enabled host action");
    }
    let graph = upstream_graph(state, owner)?;
    let route = graph.routes.iter().find(|route|
        route.actor_ref == target.coworker_ref && route.role_ref == target.role_ref
        && route.action_name == target.action_name && route.actor_integration_ref == target.binding_ref
    ).ok_or_else(|| invalid_error("exact action target is unavailable"))?;
    if route_action_target(&state.graph.runtime_ref, route) != *target {
        return invalid("tool contract changed; discover the current action target");
    }
    if route.actor_ref == INTRINSIC_CWR_ACTOR_REF && route.action_name == "reload--graph"
        && (!state.allow_reconfiguration || !graph.actors.iter().any(|actor| actor.actor_ref == caller.target_actor_ref
            && actor.authority.iter().any(|grant| grant == "cwr:configure"))) {
        return invalid("workspace configuration capability is required");
    }
    route.require_available()?;
    if route.adapter_ref != UPSTREAM_ADAPTER_REF && !route.process_adapter_route()?.is_supported_by_local_host() {
        return invalid("exact action binding is unavailable on this host");
    }
    Ok(route.clone())
}

impl LocalRuntime {
    fn install_native_actions(
        &self, invocation: &mut AdapterInvocation, _record: &ActionInvocationRecord,
        _routes: &BTreeMap<String, ResolvedCwrActionRoute>,
    ) -> CwrResult<()> {
        crate::v2026_09::execution::install_native_tool_definitions(invocation)
    }
}
