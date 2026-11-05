use super::*;
use crate::adapter::{built_in_adapter_resource, is_builtin_adapter_resource};
use cwr_core::CwrRef;

const CORE_SCHEMAS: &[(&str, &str)] = &[
    (crate::entities::relationships::DEFINITION_TYPE, include_str!("../../../resources/base/conversations/relationship.schema.json")),
    (
        "cwp://org.coworkerprotocol/2026-10-23/schemas/native-service",
        include_str!("../../../resources/base/conversations/service.schema.json"),
    ),
    (
        crate::entities::DEFINITION_TYPE,
        include_str!("../../../resources/base/entities/definition.schema.json"),
    ),
    (
        requirements::PLAINTEXT_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/plaintext/schema.json"),
    ),
    (
        requirements::SECRET_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/secret/schema.json"),
    ),
    (
        RUNTIME_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/runtime/schema.json"),
    ),
    (
        LEGACY_LOOP_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/loop/schema.json"),
    ),
    (
        WORKSPACE_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/workspace/schema.json"),
    ),
    (
        ROOT_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/cwr/schema.json"),
    ),
    (
        "cwp://org.coworkerprotocol/2026-09/schemas/action",
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/action/schema.json"),
    ),
    (
        ACTION_INVOCATION_TYPE,
        include_str!(
            "../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/action_invocation/schema.json"
        ),
    ),
    (
        "cwp://org.coworkerprotocol/2026-09/schemas/action_response",
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/action_response/schema.json"),
    ),
    (
        ACTOR_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/actor/schema.json"),
    ),
    (
        ACTOR_INTEGRATION_TYPE,
        include_str!(
            "../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/actor_integration/schema.json"
        ),
    ),
    (
        ADAPTER_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/adapter/schema.json"),
    ),
    (
        CONVERSATION_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/conversation/schema.json"),
    ),
    (
        "cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_content",
        include_str!(
            "../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/conversation_timeline_content/schema.json"
        ),
    ),
    (
        "cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_cursor",
        include_str!(
            "../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/conversation_timeline_cursor/schema.json"
        ),
    ),
    (
        "cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_item",
        include_str!(
            "../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/conversation_timeline_item/schema.json"
        ),
    ),
    (
        "cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_connection",
        include_str!(
            "../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/conversation_timeline_connection/schema.json"
        ),
    ),
    (
        GATEWAY_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/gateway/schema.json"),
    ),
    (
        "cwp://org.coworkerprotocol/2026-09/schemas/conversation_message",
        include_str!(
            "../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/conversation_message/schema.json"
        ),
    ),
    (
        ROLE_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/role/schema.json"),
    ),
    (
        COWORKER_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/coworker/schema.json"),
    ),
    (
        INTEGRATION_TYPE,
        include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/integration/schema.json"),
    ),
];

pub(super) fn resolve_adapter(
    source: &Path,
    reference: &str,
    graph: &ResolvedGraph,
    scope_id: usize,
) -> CwrResult<ResolvedAdapter> {
    if let Some(built_in) = built_in_adapter(reference)? {
        return Ok(built_in);
    }
    let Some(target) = graph.scopes[scope_id].lookup.get(reference) else {
        return invalid(format!(
            "{}: unresolved adapter ref {reference}",
            source.display()
        ));
    };
    let ResolvedTarget::Resource(index) = target else {
        return invalid(format!(
            "{}: external adapter ref {reference} has no loaded definition",
            source.display()
        ));
    };
    let resource = &graph.resources[*index];
    let ResourceKind::Adapter(definition) = &resource.kind else {
        return invalid(format!(
            "{}: {reference} resolves to {}, not an adapter",
            source.display(),
            resource.type_ref
        ));
    };
    Ok(ResolvedAdapter {
        definition: definition.clone(),
        effective_ref: resource.effective_ref.clone(),
        source: AdapterSource::File(resource.source.clone()),
        schema_root_ref: Some(graph.scopes[resource.scope_id].root_ref.clone()),
    })
}

fn built_in_adapter(reference: &str) -> CwrResult<Option<ResolvedAdapter>> {
    let Some(built_in) = built_in_adapter_resource(reference) else {
        return Ok(None);
    };
    let definition = toml::from_str(built_in.definition())
        .map_err(|error| invalid_error(format!("invalid embedded adapter {reference}: {error}")))?;
    Ok(Some(ResolvedAdapter {
        definition,
        effective_ref: reference.to_owned(),
        source: AdapterSource::BuiltIn(built_in),
        schema_root_ref: None,
    }))
}

pub(super) fn validate_adapter_params(
    declaration: &Path,
    label: &str,
    adapter: &ResolvedAdapter,
    locator: Option<&SchemaLocator>,
    params: &Parameters,
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<()> {
    let Some(locator) = locator else {
        return Ok(());
    };
    let schema = load_schema_locator(
        declaration,
        label,
        locator,
        Some(adapter),
        adapter.schema_root_ref.as_deref(),
        resolver,
    )?;
    let validator = json_schema_validator(&schema).map_err(|error| {
        invalid_error(format!(
            "{}: {label} schema is not valid JSON Schema: {error}",
            declaration.display()
        ))
    })?;
    // JSON Schema can relate values across arbitrary branches and containers.
    // If inspection cannot read every parameter, defer the schema as a whole;
    // normal graph resolution validates the concrete value before execution.
    if !params.unresolved.is_empty() {
        return Ok(());
    }
    for error in validator.iter_errors(&params.value) {
        let instance_path = error.instance_path();
        let location = instance_path.as_str();
        let location = if location.is_empty() { "/" } else { location };
        if matches!(
            error.kind(),
            jsonschema::error::ValidationErrorKind::AdditionalProperties { .. }
        ) {
            eprintln!(
                "cwr: warning: {}: {label} contain properties unsupported by adapter {} at {location}",
                declaration.display(),
                adapter.definition.reference
            );
            continue;
        }
        return invalid(format!(
            "{}: {label} do not satisfy adapter {} at {location}",
            declaration.display(),
            adapter.definition.reference
        ));
    }
    Ok(())
}

pub(super) fn load_schema_locator(
    declaration: &Path,
    label: &str,
    locator: &SchemaLocator,
    adapter: Option<&ResolvedAdapter>,
    root_ref: Option<&str>,
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<Value> {
    let schema = match (&locator.path, &locator.reference) {
        (Some(relative), None) => match adapter.map(|adapter| &adapter.source) {
            Some(AdapterSource::BuiltIn(built_in)) => {
                let contents = built_in.schema(relative).ok_or_else(|| {
                    invalid_error(format!(
                        "embedded adapter schema path {relative} is unavailable"
                    ))
                })?;
                serde_json::from_str(contents).map_err(|error| {
                    invalid_error(format!("embedded adapter schema {relative}: {error}"))
                })?
            }
            Some(AdapterSource::File(adapter_path)) => read_json_schema(
                &schema_file(adapter_path, relative, label, resolver)?,
                label,
            )?,
            None => read_json_schema(&schema_file(declaration, relative, label, resolver)?, label)?,
        },
        (None, Some(reference)) => {
            validate_identity_ref(declaration, reference)?;
            let resolved_ref = root_ref
                .map(|root_ref| resolve_mount_ref(root_ref, reference))
                .unwrap_or_else(|| reference.clone());
            let resolved = resolver.resolve(&resolved_ref)?.ok_or_else(|| {
                invalid_error(format!(
                    "{}: {label} ref {resolved_ref} is not available from the host resource resolver",
                    declaration.display()
                ))
            })?;
            let resolved = resolved_regular_file(&resolved_ref, &resolved)?;
            read_json_schema(&resolved, label)?
        }
        _ => {
            return invalid(format!(
                "{}: {label} must contain exactly one of path or ref",
                declaration.display()
            ));
        }
    };
    json_schema_validator(&schema).map_err(|error| {
        invalid_error(format!(
            "{}: {label} is not valid JSON Schema: {error}",
            declaration.display()
        ))
    })?;
    Ok(schema)
}

pub(super) fn validate_standalone_resource(
    path: &Path,
    type_ref: &str,
    document: &Value,
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<()> {
    match type_ref {
        GATEWAY_TYPE => {
            let gateway: GatewayFile = decode(path, document.clone())?;
            if gateway.action_name.trim().is_empty()
                || gateway
                    .ingress_action_name
                    .as_deref()
                    .is_some_and(|name| name.trim().is_empty())
                || gateway
                    .delivery_action_name
                    .as_deref()
                    .is_some_and(|name| name.trim().is_empty())
            {
                return invalid("gateway action names must be nonempty");
            }
            Ok(())
        }
        ADAPTER_TYPE => {
            let adapter: AdapterFile = decode(path, document.clone())?;
            let resolved = ResolvedAdapter {
                definition: adapter.clone(),
                effective_ref: adapter.reference.clone(),
                source: AdapterSource::File(path.to_owned()),
                schema_root_ref: Some("~@".to_owned()),
            };
            for (label, locator) in [
                (
                    "coworker parameter schema",
                    adapter.coworker_params_schema.as_ref(),
                ),
                (
                    "integration parameter schema",
                    adapter.integration_params_schema.as_ref(),
                ),
                (
                    "actor integration parameter schema",
                    adapter.actor_integration_params_schema.as_ref(),
                ),
            ] {
                if let Some(locator) = locator {
                    load_schema_locator(
                        path,
                        label,
                        locator,
                        Some(&resolved),
                        resolved.schema_root_ref.as_deref(),
                        resolver,
                    )?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(super) fn validate_core_document(
    path: &Path,
    type_ref: &str,
    document: &Value,
) -> CwrResult<()> {
    let mut schemas = Vec::new();
    for (uri, contents) in CORE_SCHEMAS {
        let schema: Value = serde_json::from_str(contents).map_err(|error| {
            invalid_error(format!("invalid embedded core schema {uri}: {error}"))
        })?;
        schemas.push((*uri, schema));
    }
    let Some((_, schema)) = schemas.iter().find(|(uri, _)| *uri == type_ref) else {
        return invalid(format!(
            "{}: unsupported 2026-09 resource type {type_ref}",
            path.display()
        ));
    };
    let mut registry = jsonschema::Registry::new();
    for (uri, registered) in &schemas {
        registry = registry.add(*uri, registered).map_err(|error| {
            invalid_error(format!("cannot register core schema {uri}: {error}"))
        })?;
    }
    let registry = registry
        .prepare()
        .map_err(|error| invalid_error(format!("cannot prepare core schema registry: {error}")))?;
    let validator = jsonschema::options()
        .with_registry(&registry)
        .with_format("cwr-ref", cwr_ref_format)
        .should_validate_formats(true)
        .build(schema)
        .map_err(|error| {
            invalid_error(format!("cannot compile core schema {type_ref}: {error}"))
        })?;
    if let Some(error) = validator.iter_errors(document).next() {
        let location = error.instance_path().as_str();
        let location = if location.is_empty() { "/" } else { location };
        let detail = match error.kind() {
            jsonschema::error::ValidationErrorKind::AdditionalProperties { unexpected } => {
                format!(": unsupported fields {}", unexpected.join(", "))
            }
            _ => String::new(),
        };
        return invalid(format!(
            "{}: does not satisfy {type_ref} at {location}{detail}",
            path.display()
        ));
    }
    Ok(())
}

fn read_json_schema(path: &Path, label: &str) -> CwrResult<Value> {
    let contents = fs::read_to_string(path)
        .map_err(|error| crate::error::configuration_file_error(path, error))?;
    serde_json::from_str(&contents)
        .map_err(|error| invalid_error(format!("{}: {label} is not JSON: {error}", path.display())))
}

pub(super) fn is_builtin_adapter(reference: &str) -> bool {
    is_builtin_adapter_resource(reference)
}

pub(super) fn json_schema_validator(
    schema: &Value,
) -> Result<jsonschema::Validator, jsonschema::ValidationError<'static>> {
    jsonschema::options()
        .with_format("cwr-ref", cwr_ref_format)
        .should_validate_formats(true)
        .build(schema)
}

fn cwr_ref_format(value: &str) -> bool {
    CwrRef::parse(value).is_ok()
}

fn schema_file(
    declaration: &Path,
    relative: &str,
    label: &str,
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<PathBuf> {
    if let Some(roots) = resolver.source_roots() {
        if !(relative.starts_with("./") || relative.starts_with("../")) {
            return invalid(format!("{label} path must start with ./ or ../"));
        }
        return super::resolver::source_file(
            &declaration
                .parent()
                .ok_or_else(|| invalid_error("schema declaration has no parent"))?
                .join(relative),
            roots,
        );
    }
    resolve_regular_file(declaration, relative, label)
}

#[cfg(test)]
#[path = "tests/schema.rs"]
mod tests;
