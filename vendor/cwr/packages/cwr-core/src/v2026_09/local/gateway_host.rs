/// A connection-owned executor for one checked gateway. This is deliberately
/// separate from conversation discovery and does not publish a coworker.
struct NativeGatewayHandler {
    fingerprint: String,
    route: UpstreamRoute,
}

impl LocalRuntime {
    pub fn register_gateway_handlers(
        &self,
        owner: &str,
        handlers: Vec<super::upstream::UpstreamGatewayHandler>,
        writer: UpstreamWriter,
        deadline_stop_sender: std::sync::mpsc::SyncSender<UpstreamEvent>,
    ) -> CwrResult<String> {
        let mut state = self.lock()?;
        if state.shutting_down || handlers.is_empty() {
            return invalid("gateway handler registration is unavailable or empty");
        }
        let mut selected = Vec::new();
        let mut resources = BTreeSet::new();
        let mut unchanged_session = None;
        for handler in handlers {
            if !resources.insert(handler.resource_ref.clone()) {
                return invalid("gateway handler duplicated in registration");
            }
            let gateway = state
                .graph
                .gateways
                .iter()
                .find(|gateway| gateway.resource_ref == handler.resource_ref)
                .ok_or_else(|| invalid_error("gateway handler requires a configured resource"))?;
            let route = &gateway.route;
            if route.actor_integration_params.get("host_gateway") != Some(&Value::Bool(true)) {
                return invalid("configured gateway does not select host execution");
            }
            if route.actor_integration_params.get("execution_environment").is_some() {
                return invalid("SmolVM opt-in cannot use native host gateway execution");
            }
            if route.action_name != handler.action_name
                || route.input_schema != handler.input_schema
                || route.output_schema != handler.output_schema
            {
                return invalid("gateway handler contract differs from the configured route");
            }
            route.require_available()?;
            let fingerprint = gateway.approval_fingerprint()?;
            if let Some(registered) = state
                .gateway_handlers
                .get(&(owner.into(), handler.resource_ref.clone()))
            {
                if registered.fingerprint == fingerprint {
                    unchanged_session = Some(registered.route.provider_session_ref.clone());
                    continue;
                }
            }
            let declaration = UpstreamCoworker {
                actor_ref: route.actor_ref.clone(),
                coworker_ref: route.coworker_ref.clone(),
                handle: format!("@gateway_{}", Uuid::new_v4().simple()),
                name: route.action_title.clone(),
                actions: vec![super::upstream::UpstreamAction {
                    action_name: route.action_name.clone(),
                    title: route.action_title.clone(),
                    description: route.action_description.clone(),
                    input_schema: route.input_schema.clone(),
                    output_schema: route.output_schema.clone(),
                }],
            };
            selected.push((
                handler.resource_ref,
                gateway.approval_fingerprint()?,
                declaration,
            ));
        }
        if selected.is_empty() {
            return unchanged_session.ok_or_else(|| invalid_error("gateway registration is empty"));
        }
        let mut registry = state.upstream.clone();
        let session = registry.register_gateway(
            state.admission.runtime_root_ref(),
            owner,
            selected
                .iter()
                .map(|(_, _, declaration)| declaration.clone())
                .collect(),
        )?;
        // Gateway resources already belong to the checked graph. Do not retain
        // them as newly advertised actors or insert them into any catalog.
        state.upstream = registry;
        state.upstream_providers.insert(
            session.clone(),
            LocalUpstreamProvider {
                owner: owner.into(),
                writer,
                deadline_stop_sender,
            },
        );
        for (resource, fingerprint, declaration) in selected {
            state.gateway_handlers.insert(
                (owner.into(), resource.clone()),
                NativeGatewayHandler {
                    fingerprint,
                    route: UpstreamRoute {
                        provider_session_ref: session.clone(),
                        gateway_resource_ref: Some(resource),
                        coworker: declaration,
                    },
                },
            );
        }
        Ok(session)
    }
}

fn native_gateway_route(
    state: &LocalRuntimeState,
    caller: &ActionInvocationRecord,
    configured: &ResolvedCwrActionRoute,
) -> CwrResult<Option<ResolvedCwrActionRoute>> {
    let Some(gateway) = state.graph.gateways.iter().find(|gateway| {
        gateway.route.actor_ref == configured.actor_ref
            && gateway.route.actor_integration_ref == configured.actor_integration_ref
            && gateway.route.action_name == configured.action_name
    }) else {
        return invalid("configured gateway route is unavailable");
    };
    // The request lookup and admission use separate lock acquisitions. Reject a
    // superseded contract before selecting either its native or Shell executor;
    // never combine an old input/output contract with a current registration.
    if &gateway.route != configured {
        return invalid("configured gateway route changed before admission");
    }
    if configured.actor_integration_params.get("host_gateway") == Some(&Value::Bool(true))
        && configured
            .actor_integration_params
            .get("execution_environment")
            .is_some()
    {
        return invalid("SmolVM opt-in cannot use native host gateway execution");
    }
    let owner = state
        .host_local_invocation_owners
        .get(&caller.invocation_ref);
    let handler = owner.and_then(|owner| {
        state
            .gateway_handlers
            .get(&(owner.clone(), gateway.resource_ref.clone()))
    });
    let Some(handler) = handler else {
        if configured.actor_integration_params.get("host_gateway") == Some(&Value::Bool(true))
            || state
                .gateway_handlers
                .keys()
                .any(|(_, resource)| resource == &gateway.resource_ref)
        {
            return invalid("gateway handler is unavailable to this connection");
        }
        return Ok(None);
    };
    if gateway.approval_fingerprint()? != handler.fingerprint {
        return invalid("configured gateway changed after handler registration");
    }
    let mut route = configured.clone();
    route.adapter_ref = UPSTREAM_ADAPTER_REF.into();
    route.actor_integration_ref = format!(
        "{}/routes/{}",
        handler.route.provider_session_ref,
        handler.route.coworker.handle.trim_start_matches('@')
    );
    route.integration_ref = handler.route.provider_session_ref.clone();
    route.adapter_params = json!({"upstream_route":handler.route});
    Ok(Some(route))
}

// Run under the same lock as successful result admission/publication. Context
// action grants cover the full requester chain; native gateways additionally
// retain their configured-route check.
fn require_native_gateway_result_current(
    state: &LocalRuntimeState,
    record: &ActionInvocationRecord,
) -> CwrResult<()> {
    // Action-only context grants and their full requester ancestry must still
    // hold when a successful result is delivered, including protected children.
    state.admission.check_context_execution(&state.graph, record)?;

    if retained_upstream_route(state, &record.invocation_ref)
        .is_some_and(|route| route.gateway_resource_ref.is_some())
    {
        require_current_request_authority(state, record)?;
    }
    Ok(())
}

fn require_native_gateway_publication_current(
    state: &LocalRuntimeState,
    record: &ActionInvocationRecord,
) -> CwrResult<()> {
    require_native_gateway_result_current(state, record)?;
    if let Some(guards) = state.protected_upstream_results.get(&record.invocation_ref) {
        for invocation in guards.keys() {
            let child = state
                .admission
                .invocation_record(invocation)
                .ok_or_else(|| invalid_error("protected gateway result is unavailable"))?;
            require_native_gateway_result_current(state, child)?;
        }
    }
    Ok(())
}
