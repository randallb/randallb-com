fn validate_cloud_route_inputs(
    route: &ResolvedProcessAdapterRoute,
    invocation: &AdapterInvocation,
) -> CwrResult<()> {
    if route.adapter_ref != OPEN_RESPONSES_ADAPTER_REF && route.adapter_ref != LOOPBACK_ADAPTER_REF
    {
        return protocol_failure(
            "Cloud execution currently supports Open Responses and loopback adapters",
        );
    }
    if invocation.continuation.is_some()
        || invocation
            .local_inputs
            .as_ref()
            .is_some_and(|inputs| !inputs.dependencies.is_empty())
        || route
            .integration_params
            .as_object()
            .is_none_or(|params| params.keys().any(|key| key != "endpoint"))
        || route
            .actor_integration_params
            .as_object()
            .is_none_or(|params| {
                params
                    .keys()
                    .any(|key| key != "execution_environment" && key != "cwr_action_actor_refs")
            })
    {
        return protocol_failure(
            "Cloud execution requires replayed CWR context and gateway-only inputs",
        );
    }
    Ok(())
}

fn resolve_cloud_route(
    route: &ResolvedProcessAdapterRoute,
    invocation: &AdapterInvocation,
) -> CwrResult<LocalAdapterResolution> {
    validate_cloud_route_inputs(route, invocation)?;
    let mut process = ResolvedProcessAdapter::new(sibling_adapter_executable(
        "CWR_CLOUD_ADAPTER_EXECUTABLE",
        "cwr-adapter-cloud",
    )?)
    .with_current_directory("/")
    .with_environment("CWR_CLOUD_ADAPTER_REF", &route.adapter_ref)
    .with_environment("CWR_CLOUD_ROUTE_FINGERPRINT", route.fingerprint())
    .with_shutdown_grace_period(Duration::from_secs(5));
    for name in [
        "CWR_CLOUD_COORDINATOR_URL",
        "CWR_CLOUD_COORDINATOR_TOKEN",
        "CWR_CLOUD_WORKSPACE_REF",
    ] {
        let value = std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| protocol_error(format!("Cloud execution requires {name}")))?;
        process = process.with_environment(name, value);
    }
    Ok(LocalAdapterResolution {
        process,
        integration_params: None,
    })
}

/// Reuse ordinary terminal validation before a remote result becomes durable.
/// Full process ordering and host-call admission remain with guarded transport.
pub(crate) fn validate_cloud_callback(
    invocation: &AdapterInvocation,
    event: &AdapterEvent,
) -> CwrResult<()> {
    match event {
        AdapterEvent::Result {
            invocation_ref,
            sequence,
            status,
            summary,
            result,
            error,
            continuation,
        } => {
            if invocation_ref != &invocation.invocation_ref
                || summary.trim().is_empty()
                || continuation.is_some()
            {
                return protocol_failure(
                    "Cloud result must retain invocation identity and replayable context",
                );
            }
            validate_terminal(
                &AdapterTerminal {
                    invocation_ref: invocation_ref.clone(),
                    sequence: *sequence,
                    status: *status,
                    summary: summary.clone(),
                    result: result.clone(),
                    error: error.clone(),
                    continuation: None,
                },
                &invocation.output_schema,
            )
        }
        AdapterEvent::Progress {
            status,
            summary,
            progress,
            ..
        } => {
            if *status != 102 || summary.trim().is_empty() {
                return protocol_failure("Invalid cloud progress");
            }
            if let Some(progress) = progress {
                progress.validate().map_err(protocol_error)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn cloud_transport_readiness() -> AdapterReadiness {
    let checked = (|| -> CwrResult<()> {
        sibling_adapter_executable("CWR_CLOUD_ADAPTER_EXECUTABLE", "cwr-adapter-cloud")?;
        let endpoint = std::env::var("CWR_CLOUD_COORDINATOR_URL")
            .map_err(|_| protocol_error("Cloud coordinator origin is missing"))?;
        let endpoint = reqwest::Url::parse(&endpoint)?;
        if (endpoint.scheme() != "https"
            && !(endpoint.scheme() == "http"
                && matches!(
                    endpoint.host_str(),
                    Some("127.0.0.1" | "localhost" | "[::1]")
                )))
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.path() != "/"
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return protocol_failure("Invalid trusted cloud coordinator origin");
        }
        let token = std::env::var("CWR_CLOUD_COORDINATOR_TOKEN")
            .map_err(|_| protocol_error("Cloud coordinator credential is missing"))?;
        if token.len() < 32 {
            return protocol_failure("Cloud coordinator credential is incomplete");
        }
        let workspace = std::env::var("CWR_CLOUD_WORKSPACE_REF")
            .map_err(|_| protocol_error("Cloud workspace binding is missing"))?;
        CwrRef::parse(&workspace)?;
        Ok(())
    })();
    match checked {
        Ok(()) => adapter_readiness(
            AdapterReadinessStatus::Ready,
            "ready",
            "Workloop Cloud execution is configured.",
        ),
        Err(_) => adapter_readiness(
            AdapterReadinessStatus::SetupRequired,
            "cloud_setup_required",
            "Workloop Cloud execution is not configured for this workspace.",
        ),
    }
}

#[cfg(test)]
mod cloud_readiness_tests {
    use super::*;
    #[test]
    fn cloud_readiness_uses_only_its_proxy_and_trusted_host_configuration() {
        let names = [
            "CWR_CLOUD_ADAPTER_EXECUTABLE",
            "CWR_CLOUD_COORDINATOR_URL",
            "CWR_CLOUD_COORDINATOR_TOKEN",
            "CWR_CLOUD_WORKSPACE_REF",
        ];
        let previous: Vec<_> = names
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("proxy");
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let route = ResolvedProcessAdapterRoute::new(
            OPEN_RESPONSES_ADAPTER_REF,
            "/",
            "/",
            json!({"model":"fixture"}),
            json!({"endpoint":"https://provider.invalid/responses"}),
            json!({"execution_environment":"workloop_cloud"}),
        );
        std::env::set_var(names[0], &executable);
        std::env::remove_var(names[1]);
        std::env::set_var(names[2], "private-fixture-token-at-least-thirty-two-bytes");
        std::env::set_var(names[3], "cwp://fixture.test/workspaces/one");
        assert!(!route.readiness().is_ready());
        std::env::set_var(names[1], "http://public.invalid");
        assert!(!route.readiness().is_ready());
        std::env::set_var(names[1], "https://coordinator.invalid");
        assert!(route.readiness().is_ready());
        std::env::set_var(names[0], directory.path().join("missing"));
        assert!(!route.readiness().is_ready());
        for (name, value) in previous {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
    }
}
