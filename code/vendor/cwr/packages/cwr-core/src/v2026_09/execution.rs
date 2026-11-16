//! Execution contracts shared by conversation-owning hosts.
use super::{
    graph::{ResolvedCwrActionRoute, ResolvedCwrGraph},
    invocation::ActionInvocationRecord,
};
use crate::{
    action_contract::ExactActionTarget,
    adapter::{AdapterHostOperation, AdapterInvocation},
    CwrResult,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn invalid_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    message.into().into()
}
fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(invalid_error(message))
}

/// Compile selections after the host sets the configured runtime identity.
/// Admitted work retains these exact names, targets, and schemas across activation.
pub fn retain_native_tool_definitions(graph: &mut ResolvedCwrGraph) -> CwrResult<()> {
    let mut definitions = BTreeMap::new();
    for actor in &graph.actors {
        let mut operations = Vec::new();
        let mut names = BTreeSet::new();
        for selection in &actor.native_tools {
            let route = graph
                .routes
                .iter()
                .find(|route| {
                    route.actor_ref == selection.coworker_ref
                        && route.action_name == selection.action_name
                        && selection.binding_ref.as_deref()
                            == Some(route.actor_integration_ref.as_str())
                })
                .ok_or_else(|| invalid_error("selected native action is unavailable"))?;
            let target_actor = graph
                .actors
                .iter()
                .find(|actor| actor.actor_ref == route.actor_ref)
                .ok_or_else(|| invalid_error("native tool target is unavailable"))?;
            let name = format!("cwp.{}--{}", target_actor.coworker_slug, route.action_name);
            let logical_name = crate::tool_names::LogicalToolName::for_host_operation(&name)?;
            for binding in graph
                .actor_integrations
                .iter()
                .filter(|binding| binding.actor_ref == actor.actor_ref)
            {
                let encoded_length = match binding.adapter_ref.as_str() {
                    crate::adapter::CLAUDE_ADAPTER_REF => logical_name.local_name.len(),
                    crate::adapter::OPEN_RESPONSES_ADAPTER_REF => logical_name.flat_name().len(),
                    _ => continue,
                };
                if encoded_length > 64 {
                    return invalid(format!(
                        "native tool {name} encodes to {encoded_length} bytes for caller binding {}; adapter {} permits at most 64 bytes",
                        binding.actor_integration_ref, binding.adapter_ref
                    ));
                }
            }
            if !names.insert(name.clone()) {
                return invalid(format!("ambiguous native tool name {name}"));
            }
            let mut schema = route.input_schema.clone();
            schema
                .as_object_mut()
                .ok_or("native tool input contract must be an object schema")?
                .insert(
                    "x-cwr-target".into(),
                    serde_json::to_value(route_action_target(&graph.runtime_ref, route))?,
                );
            operations.push(AdapterHostOperation {
                name,
                description: route.action_description.clone(),
                input_schema: schema,
                output_schema: crate::action_contract::invoke_action_output_schema(),
            });
        }
        definitions.insert(actor.actor_ref.clone(), serde_json::to_value(operations)?);
    }
    for binding in &mut graph.actor_integrations {
        binding.adapter_params["cwr_native_tools"] = definitions
            .get(&binding.actor_ref)
            .ok_or("native tool binding coworker is unavailable")?
            .clone();
    }
    for route in &mut graph.routes {
        route.adapter_params["cwr_native_tools"] = definitions
            .get(&route.actor_ref)
            .ok_or("native tool route coworker is unavailable")?
            .clone();
    }
    Ok(())
}

#[cfg(test)]
mod tests;

/// Add only the native definitions captured in this invocation's activated route.
/// Hosts call this while seeding a new toolset and retain that toolset on retries.
pub fn install_native_tool_definitions(invocation: &mut AdapterInvocation) -> CwrResult<()> {
    let retained: Vec<AdapterHostOperation> = invocation
        .adapter_params
        .get("cwr_native_tools")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()?
        .unwrap_or_default();
    for operation in retained {
        if invocation
            .host_operations
            .iter()
            .any(|existing| existing.name == operation.name)
        {
            return invalid(format!("ambiguous native tool name {}", operation.name));
        }
        invocation.host_operations.push(operation);
    }
    for operation in &mut invocation.host_operations {
        let name = crate::tool_names::LogicalToolName::for_host_operation(&operation.name)?;
        operation.input_schema["x-cwr-model-name"] = json!(name.qualified());
        operation.input_schema["x-cwr-decoder-version"] = json!("cwp/1");
    }
    Ok(())
}

/// Select the admitted provider configuration while enforcing current binding grants.
/// A host must separately recheck the requester's permission to use this binding.
pub fn selected_process_route(
    graph: &ResolvedCwrGraph,
    record: &ActionInvocationRecord,
    invocation: &mut AdapterInvocation,
) -> CwrResult<crate::adapter::ResolvedProcessAdapterRoute> {
    let binding_ref = record
        .actor_integration_ref
        .as_deref()
        .ok_or_else(|| invalid_error("invocation has no execution binding"))?;
    let binding = graph.actor_integrations.iter().find(|binding| {
        binding.actor_ref == record.target_actor_ref && binding.actor_integration_ref == binding_ref
    });
    // Gateway handlers have an execution binding, but are not coworker actors.
    // They are selected by their retained gateway instance identity instead of
    // entering ordinary action discovery or conversation routing.
    if binding.is_none() {
        let mut gateway_routes = graph
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
            .filter(|route| {
                route.actor_ref == record.target_actor_ref
                    && route.actor_integration_ref == binding_ref
                    && route.action_name == record.action_name
            });
        let gateway_route = gateway_routes
            .next()
            .ok_or_else(|| invalid_error("admitted execution binding is unavailable"))?;
        // Several request contracts may intentionally share one legacy
        // handler. They are unambiguous when they resolve to the exact same
        // execution route; distinct routes must never substitute for one
        // another during retry.
        if gateway_routes.any(|candidate| candidate != gateway_route) {
            return invalid("admitted gateway execution binding is ambiguous");
        }
        gateway_route.require_available()?;
        if invocation
            .authority
            .iter()
            .any(|grant| !gateway_route.available_authority.contains(grant))
        {
            return invalid("admitted execution authority was revoked");
        }
        backfill_integration_ref(invocation, &gateway_route.integration_ref)?;
        let route = match &invocation.retained_route {
            Some(route) => route.clone(),
            None => gateway_route.process_adapter_route()?,
        };
        if invocation.route_fingerprint.as_deref() != Some(route.fingerprint().as_str()) {
            return invalid(
                "retained execution configuration does not match the admitted invocation",
            );
        }
        invocation.retained_route = Some(route.clone());
        return Ok(route);
    }
    let binding = binding.expect("checked above");
    binding.require_available()?;
    // Current grants remain live even when the admitted provider configuration
    // is older. Configuration changes cannot add authority to queued work.
    if invocation
        .authority
        .iter()
        .any(|grant| !binding.available_authority.contains(grant))
    {
        return invalid("admitted execution authority was revoked");
    }
    backfill_integration_ref(invocation, &binding.integration_ref)?;
    let route = match &invocation.retained_route {
        Some(route) => route.clone(),
        // Older snapshots retained the fingerprint without the route itself.
        // Recover only an exact match; a changed binding cannot replace it.
        None => binding.process_adapter_route()?,
    };
    if invocation.route_fingerprint.as_deref() != Some(route.fingerprint().as_str()) {
        return invalid("retained execution configuration does not match the admitted invocation");
    }
    invocation.retained_route = Some(route.clone());
    Ok(route)
}

fn backfill_integration_ref(
    invocation: &mut AdapterInvocation,
    integration_ref: &str,
) -> CwrResult<()> {
    match invocation.integration_ref.as_deref() {
        Some(retained) if retained != integration_ref => {
            invalid("retained invocation has another provider integration ref")
        }
        Some(_) => Ok(()),
        None => {
            invocation.integration_ref = Some(integration_ref.into());
            Ok(())
        }
    }
}

/// Capture an exact action binding and its schemas and execution semantics.
pub fn route_action_target(runtime_ref: &str, route: &ResolvedCwrActionRoute) -> ExactActionTarget {
    let contract = json!({
        "input": route.input_schema, "output": route.output_schema,
        "execution_scope": route.execution_scope, "response_mode": route.response_mode,
        "revision": route.contract_revision,
    });
    ExactActionTarget {
        runtime_ref: runtime_ref.into(),
        coworker_ref: route.actor_ref.clone(),
        role_ref: route.role_ref.clone(),
        action_name: route.action_name.clone(),
        binding_ref: route.actor_integration_ref.clone(),
        contract_id: format!(
            "sha256:{}",
            hex::encode(Sha256::digest(contract.to_string().as_bytes()))
        ),
    }
}
