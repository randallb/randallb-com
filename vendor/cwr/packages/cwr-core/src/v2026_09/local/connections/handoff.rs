//! The guard lives through process creation and the complete invoke-frame write.
use super::*;
use crate::{
    adapter::{ProcessAdapterStopControl, ResolvedProcessAdapterRoute},
    credentials::SecretStore,
    input_delivery::{InputDependency, InvocationInputs},
};

pub(crate) struct InputHandoff {
    _grants: Vec<File>,
    _environment: Option<EnvironmentAccess>,
    _legacy: Option<LegacyAccess>,
}

struct PreparedValue {
    value: String,
    metadata: Option<crate::workspace_environment::EntryMetadata>,
}

pub(super) struct ApprovedBinding {
    pub(super) integration_ref: String,
    pub(super) input_plan: InvocationInputs,
    pub(super) input_uses: Vec<crate::input_delivery::InputUse>,
    pub(super) route: ResolvedProcessAdapterRoute,
}

pub(super) fn approved_binding(
    graph: &ResolvedCwrGraph,
    binding_ref: &str,
) -> CwrResult<ApprovedBinding> {
    if let Some(binding) = graph
        .actor_integrations
        .iter()
        .find(|binding| binding.actor_integration_ref == binding_ref)
    {
        return Ok(ApprovedBinding {
            integration_ref: binding.integration_ref.clone(),
            input_plan: binding.input_plan.clone(),
            input_uses: binding.input_uses.clone(),
            route: binding.process_adapter_route()?,
        });
    }
    let mut routes = graph
        .gateways
        .iter()
        .flat_map(|gateway| {
            [
                Some(&gateway.route),
                gateway.ingress_route.as_ref(),
                gateway.delivery_route.as_ref(),
            ]
        })
        .flatten()
        .filter(|route| route.actor_integration_ref == binding_ref);
    let first = routes
        .next()
        .ok_or("The approved binding is unavailable.")?;
    let first_process = first.process_adapter_route()?;
    // Entry points have different action contracts but share one execution binding.
    for route in routes {
        if route.integration_ref != first.integration_ref
            || route.input_plan != first.input_plan
            || route.input_uses != first.input_uses
            || !route
                .process_adapter_route()?
                .same_input_template(&first_process)
        {
            return invalid("The approved gateway binding is ambiguous.");
        }
    }
    Ok(ApprovedBinding {
        integration_ref: first.integration_ref.clone(),
        input_plan: first.input_plan.clone(),
        input_uses: first.input_uses.clone(),
        route: first_process,
    })
}

pub(super) fn retained_inputs(
    route: &ResolvedProcessAdapterRoute,
    invocation: &AdapterInvocation,
) -> CwrResult<InvocationInputs> {
    match &invocation.local_inputs {
        Some(plan) => Ok(plan.clone()),
        None if !route.has_unresolved_inputs(invocation) => Ok(InvocationInputs::default()),
        None => Err("Input approval is unavailable. Start this work again.".into()),
    }
}

pub(crate) fn prepare_handoff(
    route: &mut ResolvedProcessAdapterRoute,
    invocation: &mut AdapterInvocation,
    stop: &ProcessAdapterStopControl,
) -> CwrResult<Option<InputHandoff>> {
    prepare_with(route, invocation, stop, |id| {
        crate::workspace_environment::workspace_secret_store()?
            .get(id)
            .map_err(|_| {
                "The workspace secret is unavailable. Unlock Keychain or replace it.".into()
            })
            .and_then(|value| {
                value.ok_or_else(|| {
                    "The workspace secret is unavailable. Unlock Keychain or replace it.".into()
                })
            })
    })
}

pub(super) fn prepare_with(
    route: &mut ResolvedProcessAdapterRoute,
    invocation: &mut AdapterInvocation,
    stop: &ProcessAdapterStopControl,
    mut secret: impl FnMut(&str) -> CwrResult<String>,
) -> CwrResult<Option<InputHandoff>> {
    let plan = retained_inputs(route, invocation)?;
    if plan.version != 1 {
        return changed();
    }
    if plan.dependencies.is_empty() {
        return Ok(None);
    }
    let context = plan
        .context
        .as_ref()
        .ok_or("Input approval is incomplete. Start this work again.")?;
    if context.storage_identity != crate::workspace_environment::storage_identity()? {
        return changed();
    }
    let target = target_for_context(context)?;
    let options = options_for_target(&target)?;
    let graph = graph::resolve_bound_graph(
        &target.path,
        &options,
        graph::requirements::concrete_bindings(&options.inputs)?,
    )?;
    let binding = approved_binding(&graph, &plan.binding_ref)?;
    if capture_for_binding(&binding.input_plan, &binding.input_uses)? != plan
        || Some(&binding.integration_ref) != invocation.integration_ref.as_ref()
        || !route.same_input_template(&binding.route)
        || !route.matches_invocation_parameters(invocation)
    {
        return changed();
    }
    let mut prepared = Vec::new();
    {
        let guard = lock_stores(&target, &plan, stop)?;
        validate_grants(&target, &plan)?;
        for dependency in &plan.dependencies {
            let value = match &dependency.selection {
                InputSelection::Workspace {
                    reference,
                    revision,
                    entry_kind,
                    ..
                } => {
                    let db = guard
                        ._environment
                        .as_ref()
                        .ok_or("Missing environment access.")?;
                    let metadata = db.metadata(reference)?;
                    if metadata.revision != *revision || metadata.kind != *entry_kind {
                        return changed();
                    }
                    let value = if *entry_kind == EnvironmentEntryKind::Variable {
                        db.variable(reference)?
                    } else {
                        String::new()
                    };
                    PreparedValue {
                        value,
                        metadata: Some(metadata),
                    }
                }
                InputSelection::Legacy { key_id, .. } => PreparedValue {
                    value: guard
                        ._legacy
                        .as_ref()
                        .ok_or("Missing credential access.")?
                        .value(key_id)?,
                    metadata: None,
                },
                InputSelection::Absent => PreparedValue {
                    value: String::new(),
                    metadata: None,
                },
                InputSelection::Unavailable => return changed(),
            };
            prepared.push(value);
        }
    }
    // No coordination is held during native credential access. Recheck below
    // rejects any replacement/removal that completes while the provider blocks.
    for value in &mut prepared {
        if let Some(id) = value
            .metadata
            .as_ref()
            .and_then(|m| m.credential_id.as_deref())
        {
            value.value = secret(id)?;
        }
    }
    // Source reads can wait on an unavailable workspace mount. Read them before
    // coordination; source edits do not participate in grant/store locking.
    let definition = definition_for_target(&target)?;
    if definition.fingerprint != context.definition
        || approved_binding(&definition.graph, &plan.binding_ref).is_err()
    {
        return changed();
    }
    let guard = lock_stores(&target, &plan, stop)?;
    validate_grants(&target, &plan)?;
    for (dependency, value) in plan.dependencies.iter().zip(&prepared) {
        match &dependency.selection {
            InputSelection::Workspace { reference, .. } => {
                if Some(
                    guard
                        ._environment
                        .as_ref()
                        .ok_or("Missing environment access.")?
                        .metadata(reference)?,
                ) != value.metadata
                {
                    return changed();
                }
            }
            InputSelection::Legacy { key_id, .. } => {
                if guard
                    ._legacy
                    .as_ref()
                    .ok_or("Missing credential access.")?
                    .value(key_id)?
                    != value.value
                {
                    return changed();
                }
            }
            InputSelection::Absent => {}
            InputSelection::Unavailable => return changed(),
        }
    }
    if stop.is_requested() {
        return Err("Input operation cancelled.".into());
    }
    // Apply to private copies only after every dependency has passed validation.
    let mut private_route = route.clone();
    let mut private_invocation = invocation.clone();
    for (dependency, value) in plan.dependencies.iter().zip(prepared) {
        for destination in &dependency.destinations {
            if dependency.selection == InputSelection::Absent {
                private_route.omit_input(destination)?;
                crate::adapter::omit_input(
                    &mut private_invocation.integration_params,
                    destination,
                )?;
                continue;
            }
            private_route.materialize_input(destination, &value.value)?;
            let slot = private_invocation
                .integration_params
                .pointer_mut(&destination.pointer())
                .ok_or("Input destination changed.")?;
            *slot = json!(value.value);
        }
    }
    *route = private_route;
    *invocation = private_invocation;
    Ok(Some(guard))
}

fn lock_stores(
    target: &ConnectionTarget,
    plan: &InvocationInputs,
    stop: &ProcessAdapterStopControl,
) -> CwrResult<InputHandoff> {
    let grants = acquire_grants_lock_with(&target.grants_path, || stop.is_requested())?;
    let environment = plan
        .dependencies
        .iter()
        .any(|d| matches!(d.selection, InputSelection::Workspace { .. }))
        .then(|| EnvironmentAccess::open_canonical(&target.workspace, || stop.is_requested()))
        .transpose()?;
    let legacy = plan
        .dependencies
        .iter()
        .any(|d| matches!(d.selection, InputSelection::Legacy { .. }))
        .then(|| LegacyAccess::open(|| stop.is_requested()))
        .transpose()?;
    Ok(InputHandoff {
        _grants: grants,
        _environment: environment,
        _legacy: legacy,
    })
}

fn validate_grants(target: &ConnectionTarget, plan: &InvocationInputs) -> CwrResult<()> {
    let grants = read_grants(&target.grants_path)?;
    let context = plan.context.as_ref().ok_or("Missing input context.")?;
    for InputDependency {
        source, selection, ..
    } in &plan.dependencies
    {
        if source.scope != target.path.to_string_lossy() {
            return changed();
        }
        if *selection == InputSelection::Absent {
            if grants
                .connections
                .get(&source.alias)
                .is_some_and(|g| g.fingerprint == context.definition)
            {
                return changed();
            }
            continue;
        }
        let grant = grants
            .connections
            .get(&source.alias)
            .ok_or("Input access changed. Start this work again.")?;
        let valid = grant.fingerprint == context.definition
            && match selection {
                InputSelection::Workspace {
                    reference,
                    generation,
                    ..
                } => {
                    grant.backend == Backend::Workspace
                        && grant.key_id == *reference
                        && grant.token == *generation
                }
                InputSelection::Legacy { key_id, generation } => {
                    grant.backend == Backend::Legacy
                        && grant.key_id == *key_id
                        && grant.token == *generation
                }
                InputSelection::Absent | InputSelection::Unavailable => false,
            };
        if !valid {
            return changed();
        }
    }
    Ok(())
}

fn changed<T>() -> CwrResult<T> {
    Err("Input settings changed. Start this work again.".into())
}
