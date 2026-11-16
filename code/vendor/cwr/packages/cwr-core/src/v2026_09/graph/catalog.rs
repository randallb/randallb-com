fn validate_graph(
    graph: &ResolvedGraph,
    resolver: &dyn CheckResourceResolver,
    enforce_destinations: bool,
) -> CwrResult<(
    Vec<ResolvedCwrActor>,
    Vec<ResolvedCwrActorIntegration>,
    Vec<ResolvedCwrActionRoute>,
)> {
    validate_graph_for_loading(
        graph,
        resolver,
        enforce_destinations,
        false,
        &mut Vec::new(),
    )
}

fn validate_graph_for_loading(
    graph: &ResolvedGraph,
    resolver: &dyn CheckResourceResolver,
    enforce_destinations: bool,
    allow_partial: bool,
    warnings: &mut Vec<GraphLoadWarning>,
) -> CwrResult<(
    Vec<ResolvedCwrActor>,
    Vec<ResolvedCwrActorIntegration>,
    Vec<ResolvedCwrActionRoute>,
)> {
    let mut invalid_roles = BTreeSet::new();
    // A shared role is checked once, even when many coworkers select it.
    for resource in &graph.resources {
        if graph.disabled.contains(&resource.source) {
            continue;
        }
        if let ResourceKind::Role(role) = &resource.kind {
            let mut checked_role = role.clone();
            if allow_partial {
                // Adapter settings belong to each execution binding, not the whole role.
                checked_role.adapters.clear();
            }
            match validate_role(resource, &checked_role, graph, resolver) {
                Ok(()) => {}
                Err(_error) if allow_partial => {
                    invalid_roles.insert(resource.source.clone());
                    warnings.push(GraphLoadWarning {
                        source: resource.source.clone(),
                        message: "Role disabled because its configuration is invalid.".into(),
                    });
                }
                Err(error) => return Err(error),
            }
        }
    }
    let mut actors = Vec::new();
    let mut bindings = Vec::new();
    let mut routes = Vec::new();
    for resource in &graph.resources {
        let ResourceKind::Coworker(coworker) = &resource.kind else {
            continue;
        };
        if graph.disabled.contains(&resource.source) {
            continue;
        }
        let resolved = (|| -> CwrResult<_> {
            let slug = coworker
                .slug
                .clone()
                .unwrap_or_else(|| inferred_coworker_slug(coworker));
            let roles = selected_roles(resource, coworker, graph)?;
            if roles
                .iter()
                .any(|(role_resource, _)| invalid_roles.contains(&role_resource.source))
            {
                return invalid("required role is disabled");
            }
            let default = default_binding(resource, coworker)?;
            let default_target = match default_conversation_target(resource, coworker, graph) {
                Ok(target) => target,
                Err(_error) if allow_partial => {
                    warnings.push(GraphLoadWarning {
                        source: resource.source.clone(),
                        message: "The default coworker is unavailable. Choose another coworker."
                            .into(),
                    });
                    None
                }
                Err(error) => return Err(error),
            };
            let mut actor = ResolvedCwrActor {
                actor_ref: resource.effective_ref.clone(),
                actor_source: resource.source.clone(),
                actor_name: coworker.name.clone(),
                authority: normalized_authority(&coworker.authority),
                native_tools: resolve_native_tools(resource, coworker, graph)?,
                role_refs: roles
                    .iter()
                    .map(|(role, _)| role.effective_ref.clone())
                    .collect(),
                message_binding_preference: message_binding_preference(
                    resource,
                    coworker,
                    default.as_deref(),
                )?,
                default_actor_integration_ref: default,
                default_conversation_target_ref: default_target,
                coworker_ref: resource.effective_ref.clone(),
                coworker_source: resource.source.clone(),
                coworker_name: coworker.name.clone(),
                coworker_slug: slug,
                category: None,
                provider_model: None,
            };
            let mut actor_bindings = Vec::new();
            let mut actor_routes = Vec::new();
            for binding in &coworker.integrations {
                let resolved = resolve_binding_routes(
                    resource,
                    coworker,
                    binding,
                    &roles,
                    graph,
                    resolver,
                    enforce_destinations,
                );
                match resolved {
                    Ok((resolved, binding_routes)) => {
                        actor_bindings.push(resolved);
                        actor_routes.extend(binding_routes);
                    }
                    Err(_error) if allow_partial => warnings.push(GraphLoadWarning {
                        source: resource.source.clone(),
                        message: format!("Binding {} is unavailable because its configuration or a required dependency is invalid.",
                            binding.locator.alias.as_deref().unwrap_or("default")),
                    }),
                    Err(error) => return Err(error),
                }
            }
            if allow_partial {
                if !coworker.integrations.is_empty() && actor_bindings.is_empty() {
                    return invalid("no usable integration bindings remain");
                }
                retain_available_bindings(&mut actor, &actor_bindings);
            }
            Ok((actor, actor_bindings, actor_routes))
        })();
        match resolved {
            Ok((actor, actor_bindings, actor_routes)) => {
                actors.push(actor);
                bindings.extend(actor_bindings);
                routes.extend(actor_routes);
            }
            Err(_error) if allow_partial => warnings.push(GraphLoadWarning {
                source: resource.source.clone(),
                message: "Coworker disabled because its configuration or a required dependency is invalid.".into(),
            }),
            Err(error) => return Err(error),
        }
    }
    retain_unique_slugs(
        &mut actors,
        &mut bindings,
        &mut routes,
        allow_partial,
        warnings,
    )?;
    if allow_partial {
        let available: BTreeSet<_> = actors.iter().map(|actor| actor.actor_ref.clone()).collect();
        for actor in &mut actors {
            retain_available_native_tools(actor, &routes, warnings);
            if actor
                .default_conversation_target_ref
                .as_ref()
                .is_some_and(|target| !available.contains(target))
            {
                actor.default_conversation_target_ref = None;
                warnings.push(GraphLoadWarning {
                    source: actor.coworker_source.clone(),
                    message: "The default coworker is unavailable. Choose another coworker.".into(),
                });
            }
        }
    }
    for actor in &mut actors {
        let default_binding =
            actor
                .default_actor_integration_ref
                .as_deref()
                .and_then(|reference| {
                    bindings
                        .iter()
                        .find(|binding| binding.actor_integration_ref == reference)
                });
        actor.category = match default_binding {
            None if bindings
                .iter()
                .any(|binding| binding.actor_ref == actor.actor_ref) =>
            {
                None
            }
            None => Some(CoworkerCategory::Human),
            Some(binding) => category_for_adapter(&binding.adapter_ref),
        };
        actor.provider_model = default_binding.and_then(provider_model_for_binding);
    }
    actors.sort_by(|a, b| a.actor_ref.cmp(&b.actor_ref));
    bindings.sort_by(|a, b| a.actor_integration_ref.cmp(&b.actor_integration_ref));
    routes.sort_by(|a, b| {
        (&a.actor_ref, &a.action_name, &a.actor_integration_ref).cmp(&(
            &b.actor_ref,
            &b.action_name,
            &b.actor_integration_ref,
        ))
    });
    validate_state_resources(graph, &actors, &routes, resolver)?;
    Ok((actors, bindings, routes))
}

fn provider_model_for_binding(binding: &ResolvedCwrActorIntegration) -> Option<String> {
    let provider = match binding.adapter_ref.as_str() {
        crate::adapter::CODEX_ADAPTER_REF => "Codex",
        crate::adapter::CLAUDE_ADAPTER_REF => "Claude",
        crate::adapter::DECISIONS_ADAPTER_REF => "Decisions",
        crate::adapter::OPEN_RESPONSES_ADAPTER_REF => "Open Responses",
        _ => return None,
    };
    let model = binding
        .adapter_params
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .unwrap_or("default");
    Some(format!("{provider} {model}"))
}

fn category_for_adapter(adapter_ref: &str) -> Option<CoworkerCategory> {
    match adapter_ref {
        crate::adapter::PERSON_ADAPTER_REF => Some(CoworkerCategory::Human),
        crate::adapter::CODEX_ADAPTER_REF
        | crate::adapter::CLAUDE_ADAPTER_REF
        | crate::adapter::DECISIONS_ADAPTER_REF
        | crate::adapter::OPEN_RESPONSES_ADAPTER_REF => Some(CoworkerCategory::Ai),
        crate::adapter::SH_ADAPTER_REF
        | crate::adapter::LOOPBACK_ADAPTER_REF
        | crate::adapter::RNG_ADAPTER_REF
        | crate::action_contract::INTRINSIC_CWR_ADAPTER_REF => Some(CoworkerCategory::Software),
        _ => None,
    }
}

fn resolve_binding_routes(
    resource: &LoadedResource,
    coworker: &CoworkerFile,
    binding: &IntegrationBinding,
    roles: &[(&LoadedResource, &RoleFile)],
    graph: &ResolvedGraph,
    resolver: &dyn CheckResourceResolver,
    enforce_destinations: bool,
) -> CwrResult<(ResolvedCwrActorIntegration, Vec<ResolvedCwrActionRoute>)> {
    let resolved = resolve_binding(
        resource,
        coworker,
        binding,
        roles,
        graph,
        resolver,
        enforce_destinations,
    )?;
    let mut routes = Vec::new();
    for (role_resource, role) in roles {
        for action in &role.actions {
            routes.push(action_route(
                action,
                role_resource,
                &resolved,
                graph,
                resolver,
            )?);
        }
    }
    Ok((resolved, routes))
}

fn retain_available_bindings(
    actor: &mut ResolvedCwrActor,
    bindings: &[ResolvedCwrActorIntegration],
) {
    let available: BTreeSet<_> = bindings
        .iter()
        .map(|binding| binding.actor_integration_ref.as_str())
        .collect();
    actor
        .message_binding_preference
        .retain(|reference| available.contains(reference.as_str()));
    if actor
        .default_actor_integration_ref
        .as_deref()
        .is_some_and(|reference| !available.contains(reference))
    {
        // Do not replace an explicitly selected default with another binding.
        actor.default_actor_integration_ref = None;
    }
}

fn retain_available_native_tools(
    actor: &mut ResolvedCwrActor,
    routes: &[ResolvedCwrActionRoute],
    warnings: &mut Vec<GraphLoadWarning>,
) {
    actor.native_tools.retain(|tool| {
        let retained = routes.iter().any(|route| {
            route.actor_ref == tool.coworker_ref
                && route.action_name == tool.action_name
                && tool.binding_ref.as_deref() == Some(route.actor_integration_ref.as_str())
        });
        if !retained {
            warnings.push(GraphLoadWarning {
                source: actor.coworker_source.clone(),
                message: "Native tool disabled because its selected binding is unavailable.".into(),
            });
        }
        retained
    });
}

fn retain_unique_slugs(
    actors: &mut Vec<ResolvedCwrActor>,
    bindings: &mut Vec<ResolvedCwrActorIntegration>,
    routes: &mut Vec<ResolvedCwrActionRoute>,
    allow_partial: bool,
    warnings: &mut Vec<GraphLoadWarning>,
) -> CwrResult<()> {
    let mut counts = BTreeMap::new();
    for actor in actors.iter() {
        *counts.entry(actor.coworker_slug.clone()).or_insert(0) += 1;
    }
    if !allow_partial && counts.values().any(|count| *count > 1) {
        return invalid("coworker slug names multiple declarations");
    }
    actors.retain(|actor| {
        let unique = counts[&actor.coworker_slug] == 1;
        if !unique {
            warnings.push(GraphLoadWarning {
                source: actor.coworker_source.clone(),
                message: "Coworker disabled because its handle names multiple declarations.".into(),
            });
        }
        unique
    });
    let available: BTreeSet<_> = actors.iter().map(|actor| actor.actor_ref.clone()).collect();
    bindings.retain(|binding| available.contains(&binding.actor_ref));
    routes.retain(|route| available.contains(&route.actor_ref));
    Ok(())
}
