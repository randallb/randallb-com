fn adapter_readiness_by_binding(
    graph: &ResolvedCwrGraph,
    required_bindings: &BTreeSet<String>,
) -> BTreeMap<String, AdapterReadiness> {
    let mut readiness = BTreeMap::new();
    let mut probes = BTreeMap::<String, AdapterReadiness>::new();
    let mut codex_routes = Vec::new();
    for binding in &graph.actor_integrations {
        if !required_bindings.contains(&binding.actor_integration_ref) {
            continue;
        }
        if binding.adapter_ref == UPSTREAM_ADAPTER_REF {
            readiness.insert(
                binding.actor_integration_ref.clone(),
                AdapterReadiness {
                    status: crate::adapter::AdapterReadinessStatus::Ready,
                    code: "ready".into(),
                    message: "The provider is ready.".into(),
                    repair: None,
                },
            );
            continue;
        }
        let route = match binding.process_adapter_route() {
            Ok(route) => route,
            Err(error) => {
                readiness.insert(
                    binding.actor_integration_ref.clone(),
                    AdapterReadiness {
                        status: crate::adapter::AdapterReadinessStatus::SetupRequired,
                        code: "adapter_route_invalid".into(),
                        message: error.to_string(),
                        repair: None,
                    },
                );
                continue;
            }
        };
        if binding.adapter_ref == crate::adapter::CODEX_ADAPTER_REF {
            if let Some(implementation_readiness) =
                crate::adapter::codex_adapter_implementation_readiness()
            {
                readiness.insert(
                    binding.actor_integration_ref.clone(),
                    implementation_readiness,
                );
            } else {
                codex_routes.push((binding.actor_integration_ref.clone(), route));
            }
            continue;
        }
        let probe_key = if matches!(
            binding.adapter_ref.as_str(),
            crate::adapter::CODEX_ADAPTER_REF
                | crate::adapter::CLAUDE_ADAPTER_REF
                | crate::adapter::LOOPBACK_ADAPTER_REF
                | crate::adapter::RNG_ADAPTER_REF
        ) {
            binding.adapter_ref.clone()
        } else {
            binding.integration_ref.clone()
        };
        let result = probes
            .entry(probe_key)
            .or_insert_with(|| route.readiness())
            .clone();
        readiness.insert(binding.actor_integration_ref.clone(), result);
    }
    if !codex_routes.is_empty() {
        let login_readiness = crate::adapter::codex_login_readiness();
        let mut catalogs = BTreeMap::new();
        for (binding_ref, route) in codex_routes {
            let result = if !login_readiness.is_ready()
                || route.codex_model_configuration().is_none()
            {
                login_readiness.clone()
            } else {
                match catalogs
                    .entry(route.working_path().to_owned())
                    .or_insert_with(|| crate::adapter::codex_model_catalog(route.working_path()))
                {
                    Ok(catalog) => crate::adapter::codex_model_readiness(&route, catalog),
                    Err(error) => error.clone(),
                }
            };
            readiness.insert(binding_ref, result);
        }
    }
    readiness
}

fn route_readiness<'a>(
    route: &ResolvedCwrActionRoute,
    readiness: &'a BTreeMap<String, AdapterReadiness>,
) -> &'a AdapterReadiness {
    &readiness[&route.actor_integration_ref]
}
