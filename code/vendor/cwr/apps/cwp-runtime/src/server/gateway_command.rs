fn execute_gateway_command(
    state: &SharedState,
    writer: &SharedWriter,
    command: RuntimeServiceCommand,
) -> CwrResult<()> {
    match command {
        RuntimeServiceCommand::SubmitAction {
            request_id,
            integration_ref,
            author_actor_ref,
            conversation_ref,
            activation,
            input,
            ..
        } => {
            let mut locked = lock_state(state)?;
            let runtime = locked.runtime.clone();
            let gateway_connection = locked.gateways.iter().find_map(|(reference, gateway)| {
                Arc::ptr_eq(&gateway.writer, writer).then(|| reference.clone())
            });
            if let Some(reference) = &gateway_connection {
                let gateway = locked.gateways.get_mut(reference).unwrap();
                if gateway.integration_ref != integration_ref {
                    return invalid("gateway action submission uses another integration");
                }
                require_gateway_fingerprint(&runtime, reference, &gateway.approval_fingerprint)?;
                require_gateway_action(
                    &runtime, reference, &author_actor_ref, &activation,
                    &upstream_owner(writer),
                )?;
                reserve_gateway_invocation(&runtime, gateway, &request_id)?;
            }
            let admission = if let Some(reference) = &gateway_connection {
                runtime.admit_gateway_action_in_upstream_scope(
                    reference,
                    integration_ref,
                    &request_id,
                    author_actor_ref,
                    conversation_ref.as_deref(),
                    &activation,
                    input,
                    Some(&upstream_owner(writer)),
                )?
            } else {
                runtime.admit_action_in_upstream_scope(
                    integration_ref,
                    &request_id,
                    author_actor_ref,
                    conversation_ref.as_deref(),
                    &activation,
                    input,
                    Some(&upstream_owner(writer)),
                )?
            };
            if let (Some(reference), Some(invocation_ref)) =
                (gateway_connection, admission.invocation_ref())
            {
                locked.gateways.get_mut(&reference).unwrap().issued
                    .insert(invocation_ref.into(), request_id.clone());
            }
            drop(locked);
            accept_admission(state, writer, request_id, admission, None)
        }
        RuntimeServiceCommand::RegisterGateway {
            request_id,
            connection_ref,
            integration_ref,
            approval_fingerprint,
            ingress_action_name,
            delivery_action_name,
            ..
        } => {
            if connection_ref.trim().is_empty() || integration_ref.trim().is_empty() {
                return invalid("gateway registration refs must be nonempty");
            }
            if approval_fingerprint.len() != 64
                || !approval_fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return invalid("gateway approval fingerprint must be lowercase SHA-256");
            }
            if ingress_action_name.is_none() && delivery_action_name.is_none() {
                return invalid("gateway registration requires ingress or delivery");
            }
            let mut locked = lock_state(state)?;
            if locked.gateways.contains_key(&connection_ref)
                || locked
                    .gateways
                    .values()
                    .any(|gateway| Arc::ptr_eq(&gateway.writer, writer))
            {
                return invalid("gateway registration already exists");
            }
            let graph = locked.runtime.graph()?;
            let mut matching = graph.gateways.iter().filter(|gateway| {
                gateway.connection_ref.as_deref() == Some(connection_ref.as_str())
            });
            let Some(configured) = matching.next() else {
                return invalid("gateway connection does not select one configured route");
            };
            if matching.next().is_some() {
                return invalid("gateway connection does not select one configured route");
            }
            if configured.approval_fingerprint()? != approval_fingerprint {
                return invalid("gateway definition differs from private approval");
            }
            if configured
                .ingress_route
                .as_ref()
                .map(|route| &route.action_name)
                != ingress_action_name.as_ref()
                || configured
                    .delivery_route
                    .as_ref()
                    .map(|route| &route.action_name)
                    != delivery_action_name.as_ref()
            {
                return invalid("gateway registration actions differ from configured route");
            }
            locked.gateways.insert(
                connection_ref,
                RegisteredGateway {
                    integration_ref,
                    approval_fingerprint,
                    writer: Arc::clone(writer),
                    issued: BTreeMap::new(),
                },
            );
            drop(locked);
            command_succeeded(writer, request_id, None, Vec::new(), None)
        }
        RuntimeServiceCommand::InspectGatewayInvocation {
            request_id,
            connection_ref,
            invocation_ref,
            invocation_kind,
            input,
            ..
        } => {
            let (runtime, integration_ref, provider_event_ref) = {
                let locked = lock_state(state)?;
                let gateway = locked
                    .gateways
                    .get(&connection_ref)
                    .ok_or_else(|| std::io::Error::other("gateway is not registered"))?;
                if !Arc::ptr_eq(&gateway.writer, writer) {
                    return invalid("gateway registration belongs to another connection");
                }
                require_gateway_fingerprint(
                    &locked.runtime,
                    &connection_ref,
                    &gateway.approval_fingerprint,
                )?;
                let event =
                    if invocation_kind == cwr::v2026_09::service::GatewayInvocationKind::Request {
                        String::new()
                    } else {
                        gateway
                            .issued
                            .get(&invocation_ref)
                            .ok_or_else(|| {
                                std::io::Error::other(
                                    "invocation was not admitted by this gateway connection",
                                )
                            })?
                            .clone()
                    };
                (
                    locked.runtime.clone(),
                    gateway.integration_ref.clone(),
                    event,
                )
            };
            let (authorized, terminal) = runtime.inspect_gateway_invocation(
                &connection_ref,
                &integration_ref,
                &provider_event_ref,
                &invocation_ref,
                invocation_kind,
                &input,
            )?;
            write_event(
                writer,
                &RuntimeServiceEvent::GatewayInvocationInspected {
                    request_id,
                    authorized,
                    terminal,
                },
            )
        }
        _ => unreachable!("gateway dispatch selects only gateway commands"),
    }
}

fn require_gateway_fingerprint(
    runtime: &cwr::v2026_09::local::LocalRuntime,
    connection_ref: &str,
    expected: &str,
) -> CwrResult<()> {
    let graph = runtime.graph()?;
    let mut matches = graph.gateways.iter().filter(|gateway| {
        gateway.connection_ref.as_deref() == Some(connection_ref)
    });
    let Some(gateway) = matches.next() else {
        return invalid("approved gateway definition is unavailable");
    };
    if matches.next().is_some() || gateway.approval_fingerprint()? != expected {
        return invalid("gateway definition differs from private approval");
    }
    Ok(())
}

fn require_gateway_action(
    runtime: &cwr::v2026_09::local::LocalRuntime,
    connection_ref: &str,
    author_actor_ref: &str,
    activation: &str,
    owner: &str,
) -> CwrResult<()> {
    let graph = runtime.graph()?;
    let gateway = graph.gateways.iter().find(|gateway| {
        gateway.connection_ref.as_deref() == Some(connection_ref)
    }).ok_or_else(|| std::io::Error::other("approved gateway definition is unavailable"))?;
    let matches = [gateway.ingress_route.as_ref(), gateway.delivery_route.as_ref()]
        .into_iter().flatten().any(|route| {
            activation == format!("gateway--{}", route.action_name)
        });
    if !matches {
        return invalid("gateway activation differs from approved entry points");
    }
    runtime.upstream_catalog_as(author_actor_ref, Some(owner))?;
    Ok(())
}

// Keep in-flight authorizations and a bounded cache of completed submissions.
// Replaying SubmitAction rebinds an evicted terminal from the retained journal.
const MAX_GATEWAY_INVOCATIONS: usize = 1024;

fn reserve_gateway_invocation(
    runtime: &LocalRuntime,
    gateway: &mut RegisteredGateway,
    request_id: &str,
) -> CwrResult<()> {
    if gateway.issued.len() < MAX_GATEWAY_INVOCATIONS
        || gateway.issued.values().any(|event| event == request_id)
    {
        return Ok(());
    }
    let mut completed = None;
    for invocation in gateway.issued.keys() {
        if runtime.terminal_execution_for_invocation(invocation)?.is_some() {
            completed = Some(invocation.clone());
            break;
        }
    }
    if let Some(invocation) = completed {
        gateway.issued.remove(&invocation);
        Ok(())
    } else {
        invalid("gateway has too many in-flight invocations; retry after one completes")
    }
}
