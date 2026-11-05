pub fn check(input: impl AsRef<Path>) -> CwrResult<Value> {
    check_with_resolver(input, &EmptyResourceResolver)
}

pub fn check_with_resolver(
    input: impl AsRef<Path>,
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<Value> {
    check_with_options(input, resolver, &GraphLoadOptions::default())
}

pub fn check_with_options(
    input: impl AsRef<Path>,
    resolver: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
) -> CwrResult<Value> {
    let mut strict_options = options.clone();
    strict_options.allow_partial = false;
    let options = &strict_options;
    let input = absolute_path(input.as_ref())?;
    let document = read_toml_json(&input)?;
    let type_ref = document_type(&input, &document)?;
    validate_core_document(&input, type_ref, &document)?;
    if matches!(
        type_ref,
        ROOT_TYPE | RUNTIME_TYPE | LEGACY_LOOP_TYPE | COWORKER_TYPE | ROLE_TYPE | GATEWAY_TYPE
    ) {
        return serde_json::to_value(check_manifest(&input, document, resolver, options)?)
            .map_err(Into::into);
    }
    let roots = resolver::source_roots(&input, resolver, options)?;
    resolver::source_file(&input, &roots)?;
    let restricted = resolver::SourceResolver {
        host: resolver,
        roots: &roots,
    };
    validate_standalone_resource(&input, type_ref, &document, &restricted)?;
    Ok(document)
}

pub fn resolve_graph(input: impl AsRef<Path>) -> CwrResult<ResolvedCwrGraph> {
    resolve_graph_with_resolver(input, &EmptyResourceResolver)
}

pub fn resolve_resource_source(input: impl AsRef<Path>, reference: &str) -> CwrResult<PathBuf> {
    resolve_resource_source_with_options(
        input,
        reference,
        &EmptyResourceResolver,
        &GraphLoadOptions::default(),
    )
}

pub fn resolve_resource_source_with_options(
    input: impl AsRef<Path>,
    reference: &str,
    resolver: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
) -> CwrResult<PathBuf> {
    let input = absolute_path(input.as_ref())?;
    let graph = resolve_manifest_graph(&input, resolver, options)?;
    let restricted = resolver::SourceResolver {
        host: resolver,
        roots: &graph.source_roots,
    };
    validate_graph(&graph, &restricted, false)?;
    let scope = &graph.scopes[graph.root_scope];
    let reference = resolve_mount_ref(&scope.root_ref, reference);
    let target = scope.lookup.get(&reference).cloned().or_else(|| {
        graph
            .resources
            .iter()
            .position(|resource| resource.effective_ref == reference)
            .map(ResolvedTarget::Resource)
    });
    match target {
        Some(ResolvedTarget::Resource(index)) => Ok(graph.resources[index].source.clone()),
        _ => invalid(format!("unresolved resource ref {reference}")),
    }
}

pub fn resolve_graph_with_resolver(
    input: impl AsRef<Path>,
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<ResolvedCwrGraph> {
    let input = absolute_path(input.as_ref())?;
    let document = read_toml_json(&input)?;
    let type_ref = document_type(&input, &document)?;
    validate_core_document(&input, type_ref, &document)?;
    if !matches!(type_ref, ROOT_TYPE | RUNTIME_TYPE | LEGACY_LOOP_TYPE) {
        return invalid(format!(
            "{}: resolved graph input must be a 2026-09 CWR.toml entry point",
            input.display()
        ));
    }
    resolve_graph_document(&input, document, resolver, &GraphLoadOptions::default())
}

fn check_manifest(
    path: &Path,
    document: Value,
    resolver: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
) -> CwrResult<CheckedGraph> {
    let resolved = resolve_graph_document(path, document, resolver, options)?;
    checked_resolved_graph(resolved, path)
}

/// Summarize a resolved graph without exposing input values or adapter parameters.
pub fn checked_resolved_graph(resolved: ResolvedCwrGraph, path: &Path) -> CwrResult<CheckedGraph> {
    let root = path
        .parent()
        .ok_or_else(|| invalid_error("CWR.toml must have a parent directory"))?;
    let canonical_root = root.canonicalize()?;
    Ok(CheckedGraph {
        gateways: resolved
            .gateways
            .into_iter()
            .map(|gateway| CheckedGateway {
                resource_ref: gateway.resource_ref,
                coworker_ref: gateway.route.coworker_ref,
                action_name: gateway.route.action_name,
                actor_integration_ref: gateway.route.actor_integration_ref,
                ingress_action_name: gateway.ingress_route.map(|route| route.action_name),
                delivery_action_name: gateway.delivery_route.map(|route| route.action_name),
                connection_ref: gateway.connection_ref,
            })
            .collect(),
        type_ref: resolved.type_ref,
        reference: resolved.reference,
        resources: resolved
            .resources
            .into_iter()
            .map(|resource| CheckedResource {
                path: resource
                    .source
                    .strip_prefix(&canonical_root)
                    .unwrap_or(&resource.source)
                    .to_string_lossy()
                    .replace('\\', "/"),
                type_ref: resource.type_ref,
                reference: resource.reference,
                visibility: resource.visibility,
            })
            .collect(),
        actors: resolved
            .actors
            .into_iter()
            .map(|actor| CheckedActor {
                actor_ref: actor.actor_ref,
                coworker_ref: actor.coworker_ref,
                default_conversation_target_ref: actor.default_conversation_target_ref,
                authority: actor.authority,
            })
            .collect(),
        actor_integrations: resolved
            .actor_integrations
            .into_iter()
            .map(|binding| CheckedActorIntegration {
                actor_ref: binding.actor_ref,
                coworker_ref: binding.coworker_ref,
                actor_integration_ref: binding.actor_integration_ref,
                integration_ref: binding.integration_ref,
                adapter_ref: binding.adapter_ref,
                available_authority: binding.available_authority,
                missing_required_authority: binding.missing_required_authority,
            })
            .collect(),
        routes: resolved
            .routes
            .into_iter()
            .map(|route| CheckedActionRoute {
                actor_ref: route.actor_ref,
                coworker_ref: route.coworker_ref,
                action_name: route.action_name,
                action_title: route.action_title,
                action_description: route.action_description,
                actor_integration_ref: route.actor_integration_ref,
                integration_ref: route.integration_ref,
                adapter_ref: route.adapter_ref,
            })
            .collect(),
    })
}

fn resolve_graph_document(
    path: &Path,
    document: Value,
    resolver: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
) -> CwrResult<ResolvedCwrGraph> {
    resolve_graph_with_input_bindings(
        path,
        document,
        resolver,
        options,
        requirements::concrete_bindings(&options.inputs)?,
    )
}

pub(crate) fn resolve_graph_with_input_bindings(
    path: &Path,
    document: Value,
    resolver: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
    inputs: requirements::InputBindings,
) -> CwrResult<ResolvedCwrGraph> {
    let manifest: ManifestFile = decode(path, document)?;
    let graph = resolver::resolve_manifest_with_bindings(path, resolver, options, inputs)?;
    project_graph(manifest.type_ref, &graph, resolver, options)
}

fn project_graph(
    type_ref: String,
    graph: &ResolvedGraph,
    resolver: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
) -> CwrResult<ResolvedCwrGraph> {
    let restricted = resolver::SourceResolver {
        host: resolver,
        roots: &graph.source_roots,
    };
    let mut warnings = graph.warnings.clone();
    let (actors, mut actor_integrations, mut routes) = validate_graph_for_loading(graph, &restricted, options.input_resolution.enforce_destinations, options.allow_partial, &mut warnings)?;
    for binding in &mut actor_integrations { binding.input_plan = options.input_resolution.project(&binding.actor_integration_ref, &binding.input_uses); }
    for route in &mut routes { route.input_plan = options.input_resolution.project(&route.actor_integration_ref, &route.input_uses); }
    let resources = graph
        .resources
        .iter()
        .filter(|resource| !graph.disabled.contains(&resource.source))
        .map(|resource| ResolvedCwrResource {
            source: resource.source.clone(),
            scope_ref: graph.scopes[resource.scope_id].root_ref.clone(),
            type_ref: resource.type_ref.clone(),
            reference: resource.effective_ref.clone(),
            visibility: resource.visibility.into(),
            document: resource.document.clone(),
        })
        .collect();
    let root_scope = &graph.scopes[graph.root_scope];

    let mut gateways = resolve_gateway_routes(
        graph,
        &restricted,
        options.input_resolution.enforce_destinations,
        options.allow_partial,
        &mut warnings,
    )?;
    for gateway in &mut gateways {
        for route in std::iter::once(&mut gateway.route)
            .chain(gateway.ingress_route.iter_mut())
            .chain(gateway.delivery_route.iter_mut())
        {
            route.input_plan = options
                .input_resolution
                .project(&route.actor_integration_ref, &route.input_uses);
        }
    }

    let root_document = graph.resources.iter()
        .find(|resource| resource.source == root_scope.source)
        .ok_or_else(|| invalid_error("entry manifest is unavailable"))?;
    let manifest: ManifestFile = decode(&root_scope.source, root_document.document.clone())?;
    Ok(ResolvedCwrGraph {
        type_ref,
        permissions: manifest.permissions,
        reference: root_scope.root_ref.clone(),
        runtime_ref: options
            .runtime_ref
            .clone()
            .unwrap_or_else(|| root_scope.root_ref.clone()),
        resources,
        actors,
        actor_integrations,
        routes,
        gateways,
        warnings,
    })
}

fn load_resource(
    source: PathBuf,
    type_ref: &str,
    document: Value,
    root_ref: &str,
    scope_id: usize,
    visibility: Visibility,
) -> CwrResult<LoadedResource> {
    let effective_ref = root_ref.to_owned();
    let resource_document = document.clone();
    let kind = match type_ref {
        COWORKER_TYPE => ResourceKind::Coworker(decode(&source, document)?),
        GATEWAY_TYPE => ResourceKind::Gateway(decode(&source, document)?),
        ROLE_TYPE => ResourceKind::Role(decode(&source, document)?),
        INTEGRATION_TYPE => ResourceKind::Integration(decode(&source, document)?),
        ADAPTER_TYPE => ResourceKind::Adapter(decode(&source, document)?),
        CONVERSATION_TYPE => ResourceKind::Conversation(decode(&source, document)?),
        ACTION_INVOCATION_TYPE => ResourceKind::ActionInvocation(decode(&source, document)?),
        _ => ResourceKind::Other,
    };
    Ok(LoadedResource {
        source,
        type_ref: type_ref.to_owned(),
        effective_ref,
        scope_id,
        visibility,
        document: resource_document,
        kind,
    })
}

/// Workspace-selected inputs, source access, and execution directory for one runtime.
/// Input defaults are annotations only and are not applied by this resolver.
#[derive(Debug, Clone)]
pub struct GraphLoadOptions {
    /// Runtime startup may omit invalid dependencies; authoring checks stay strict.
    pub allow_partial: bool,
    pub input_resolution: crate::input_delivery::InputResolution,
    pub source_roots: Vec<PathBuf>,
    pub inputs: Value,
    pub working_path: Option<PathBuf>,
    pub runtime_ref: Option<String>,
    /// Host-approved folders for volume requests in imported runtime sources.
    pub imported_volumes: Vec<ImportedVolumeBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedVolumeBinding {
    pub source: PathBuf,
    pub alias: String,
    pub path: PathBuf,
    pub access: String,
}

impl Default for GraphLoadOptions {
    fn default() -> Self {
        Self {
            allow_partial: false,
            input_resolution: Default::default(),
            source_roots: Vec::new(),
            inputs: empty_object(),
            working_path: None,
            runtime_ref: None,
            imported_volumes: Vec::new(),
        }
    }
}

pub fn resolve_graph_with_options(
    input: impl AsRef<Path>,
    resolver: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
) -> CwrResult<ResolvedCwrGraph> {
    let input = absolute_path(input.as_ref())?;
    let document = read_toml_json(&input)?;
    resolve_graph_document(&input, document, resolver, options)
}

/// Inspect configuration requirements without supplying values or activating dependencies.
pub fn inspect_input_schema(
    input: impl AsRef<Path>,
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<Option<Value>> {
    let input = absolute_path(input.as_ref())?;
    let document = read_toml_json(&input)?;
    let manifest: ManifestFile = decode(&input, document)?;
    manifest
        .input_schema
        .as_ref()
        .map(|locator| {
            load_schema_locator(
                &input,
                "configuration input schema",
                locator,
                None,
                None,
                resolver,
            )
        })
        .transpose()
}

/// Check the selected reusable definition before resolving any supplied inputs.
pub(crate) fn validate_runtime_definition(path: &Path) -> CwrResult<()> {
    let document = read_toml_json(path).map_err(|error| {
        if error.is::<crate::ConfigurationError>() {
            invalid_error(format!(
                "{}: invalid reusable runtime declaration",
                path.display()
            ))
        } else {
            error
        }
    })?;
    let type_ref = document_type(path, &document)?;
    if !matches!(type_ref, RUNTIME_TYPE | LEGACY_LOOP_TYPE | ROOT_TYPE) {
        return invalid(format!(
            "{}: configured runtime must select a reusable runtime definition ({RUNTIME_TYPE} or legacy {LEGACY_LOOP_TYPE}) or a generic CWR graph ({ROOT_TYPE})",
            path.display()
        ));
    }
    validate_core_document(path, type_ref, &document)
}

pub(crate) fn validate_workspace_document(path: &Path, document: &Value) -> CwrResult<()> {
    // Schema errors may contain input values; do not disclose those in setup diagnostics.
    validate_core_document(path, WORKSPACE_TYPE, document)
        .map_err(|_| invalid_error("invalid WORKSPACE.toml fields; expected the workspace schema"))
}

pub(crate) fn resolve_bound_graph(
    path: &Path,
    options: &GraphLoadOptions,
    inputs: requirements::InputBindings,
) -> CwrResult<ResolvedCwrGraph> {
    // The workspace has validated the root's volume mappings, or a reload will
    // require its volume contract to match the previously activated graph.
    let graph =
        resolver::load_manifest(path, &EmptyResourceResolver, options, inputs, false, false)?.graph;
    project_graph(
        graph.resources[graph.root_scope].type_ref.clone(),
        &graph,
        &EmptyResourceResolver,
        options,
    )
}

/// Validate the available graph and return its catalog projection for host checks.
/// This path never resolves host values and its result must not be used for execution.
pub(crate) fn inspect_bound_graph(
    path: &Path,
    host: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
    inputs: requirements::InputBindings,
) -> CwrResult<(requirements::InputInspection, Option<ResolvedCwrGraph>)> {
    let mut strict_options = options.clone();
    strict_options.allow_partial = false;
    inspect_bound_graph_for_loading(path, host, &strict_options, inputs)
}

/// Connection setup can inspect valid branches with ordinary JSON still unset.
/// Unset values stay abstract, so temporary placeholders cannot disable a coworker.
pub(crate) fn inspect_partial_bound_graph(
    path: &Path,
    host: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
    inputs: requirements::InputBindings,
) -> CwrResult<(requirements::InputInspection, Option<ResolvedCwrGraph>)> {
    inspect_bound_graph_for_loading(path, host, options, inputs)
}

fn inspect_bound_graph_for_loading(
    path: &Path,
    host: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
    inputs: requirements::InputBindings,
) -> CwrResult<(requirements::InputInspection, Option<ResolvedCwrGraph>)> {
    use requirements::{RequirementIssue, RequirementStatus};
    let resolver::LoadedManifest {
        graph,
        mut report,
        can_project,
    } = resolver::load_manifest(path, host, options, inputs, true, false)?;
    if !can_project {
        return Ok((report, None));
    }
    match project_graph(
        graph.resources[graph.root_scope].type_ref.clone(),
        &graph,
        host,
        options,
    ) {
        Ok(graph) => Ok((report, Some(graph))),
        Err(error) => {
            let deferred = error.is::<parameters::UnresolvedParameterConflict>();
            report.issues.push(RequirementIssue {
                scope: path.to_string_lossy().into_owned(),
                status: if deferred {
                    RequirementStatus::Unsupported
                } else {
                    RequirementStatus::Invalid
                },
                message: if deferred {
                    error.to_string()
                } else {
                    "configured runtime does not pass startup validation".into()
                },
            });
            Ok((report, None))
        }
    }
}
