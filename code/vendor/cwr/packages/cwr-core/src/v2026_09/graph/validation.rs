fn selected_roles<'a>(
    resource: &LoadedResource,
    coworker: &CoworkerFile,
    graph: &'a ResolvedGraph,
) -> CwrResult<Vec<(&'a LoadedResource, &'a RoleFile)>> {
    let mut roles = Vec::new();
    let mut names = BTreeMap::new();
    for locator in &coworker.roles {
        let reference = locator
            .reference
            .as_deref()
            .expect("resolved role relationship");
        let role_resource = resource_of_type(
            &resource.source,
            reference,
            "role",
            graph,
            resource.scope_id,
            |kind| matches!(kind, ResourceKind::Role(_)),
        )?;
        let ResourceKind::Role(role) = &role_resource.kind else {
            unreachable!()
        };
        for action in &role.actions {
            let name = action.action_name();
            if let Some(previous) = names.insert(name.clone(), role_resource.effective_ref.clone())
            {
                return invalid(format!(
                    "coworker {} has overlapping action {name} from roles {previous} and {}",
                    resource.effective_ref, role_resource.effective_ref
                ));
            }
        }
        roles.push((role_resource, role));
    }
    Ok(roles)
}

fn default_binding(
    resource: &LoadedResource,
    coworker: &CoworkerFile,
) -> CwrResult<Option<String>> {
    let mut names = BTreeSet::new();
    for binding in &coworker.integrations {
        let name = binding.locator.alias.as_deref().ok_or_else(|| {
            invalid_error(format!(
                "{}: integration binding requires a stable as name",
                resource.source.display()
            ))
        })?;
        if !names.insert(name) {
            return invalid(format!(
                "{}: duplicate integration binding {name}",
                resource.source.display()
            ));
        }
    }
    let defaults = coworker
        .integrations
        .iter()
        .filter(|binding| binding.default)
        .collect::<Vec<_>>();
    let default = match (coworker.integrations.len(), defaults.as_slice()) {
        (0, _) => None,
        (1, _) => coworker.integrations.first(),
        (_, [binding]) => Some(*binding),
        _ => {
            return invalid(format!(
                "{}: multiple integration bindings require exactly one default",
                resource.source.display()
            ));
        }
    };
    Ok(default.map(|binding| binding_ref(resource, binding)))
}

fn message_binding_preference(
    resource: &LoadedResource,
    coworker: &CoworkerFile,
    default: Option<&str>,
) -> CwrResult<Vec<String>> {
    let mut selected = Vec::new();
    for name in &coworker.message_binding_preference {
        let binding = coworker
            .integrations
            .iter()
            .find(|binding| binding.locator.alias.as_ref() == Some(name))
            .ok_or_else(|| invalid_error(format!("unknown message preference binding {name}")))?;
        if binding.message_mode != ActorIntegrationMessageMode::Conversational {
            return invalid("message preference requires conversational bindings");
        }
        let reference = binding_ref(resource, binding);
        if selected.contains(&reference) {
            return invalid("duplicate message preference binding");
        }
        selected.push(reference);
    }
    if selected
        .first()
        .is_some_and(|first| Some(first.as_str()) != default)
    {
        return invalid("message binding preference must start with the default binding");
    }
    Ok(selected)
}

fn binding_ref(resource: &LoadedResource, binding: &IntegrationBinding) -> String {
    format!(
        "{}/bindings/{}",
        resource.effective_ref,
        binding.locator.alias.as_deref().unwrap_or("default")
    )
}

fn resolve_binding(
    resource: &LoadedResource,
    coworker: &CoworkerFile,
    binding: &IntegrationBinding,
    roles: &[(&LoadedResource, &RoleFile)],
    graph: &ResolvedGraph,
    resolver: &dyn CheckResourceResolver,
    enforce_destinations: bool,
) -> CwrResult<ResolvedCwrActorIntegration> {
    let reference = binding
        .locator
        .reference
        .as_deref()
        .ok_or_else(|| invalid_error("integration relationship is unavailable"))?;
    let integration_resource = resource_of_type(
        &resource.source,
        reference,
        "integration",
        graph,
        resource.scope_id,
        |kind| matches!(kind, ResourceKind::Integration(_)),
    )?;
    let ResourceKind::Integration(integration) = &integration_resource.kind else {
        unreachable!()
    };
    let adapter_ref = integration
        .adapter
        .reference
        .as_deref()
        .expect("resolved adapter relationship");
    let adapter = resolve_adapter(
        &integration_resource.source,
        adapter_ref,
        graph,
        integration_resource.scope_id,
    )?;
    let adapter_params =
        compose_adapter_params(resource, binding, roles, &adapter, graph, resolver)?;
    let mut binding_params = normalize_action_permissions(resource, &binding.params, graph)?;
    if adapter.effective_ref == crate::adapter::SH_ADAPTER_REF {
        let params = binding_params.value.as_object_mut()
            .ok_or_else(|| invalid_error("coworker integration parameters must be an object"))?;
        if params.contains_key("activated_volumes") {
            return invalid("activated_volumes is reserved for runtime volume mappings");
        }
        if let Some(volumes) = graph.coworker_volumes.get(&resource.source) {
            params.insert("activated_volumes".into(), Value::Object(volumes.iter()
                .map(|(alias, (path, access))| (alias.clone(), serde_json::json!({
                    "path": path,
                    "access": access,
                })))
                .collect()));
        }
    }
    let required = roles
        .iter()
        .flat_map(|(_, role)| role.required_authority.iter().cloned())
        .collect::<Vec<_>>();
    let (available_authority, missing_required_authority) = route_authority(
        &required,
        &coworker.authority,
        &adapter.definition.authority_capabilities,
    );
    let input_uses = if enforce_destinations {
        managed::reject_managed_parameters(&adapter_params)?;
        managed::compile(&adapter.effective_ref, &integration.params, &binding_params)?
    } else {
        Vec::new()
    };
    let mut integration_params = integration.params.clone();
    if adapter.effective_ref == crate::adapter::SH_ADAPTER_REF
        && !integration_params.unknown_at("/env")
    {
        serialize_shell_environment_values(&mut integration_params.value)?;
    }
    validate_adapter_params(
        &integration_resource.source,
        "integration parameters",
        &adapter,
        adapter.definition.integration_params_schema.as_ref(),
        &integration_params,
        resolver,
    )?;
    if adapter.effective_ref == crate::adapter::DECISIONS_ADAPTER_REF
        && !integration_params.unknown_at("/endpoint")
    {
        if let Some(endpoint) = integration_params.value.get("endpoint").and_then(Value::as_str) {
            crate::adapter::decisions_endpoint(endpoint).map_err(|error| invalid_error(format!(
                "{}: invalid Decisions endpoint: {error}", integration_resource.source.display()
            )))?;
        }
    }
    validate_adapter_params(
        &resource.source,
        "coworker integration parameters",
        &adapter,
        adapter.definition.actor_integration_params_schema.as_ref(),
        &binding_params,
        resolver,
    )?;
    Ok(ResolvedCwrActorIntegration {
        actor_ref: resource.effective_ref.clone(),
        actor_name: coworker.name.clone(),
        coworker_ref: resource.effective_ref.clone(),
        coworker_source: resource.source.clone(),
        working_path: graph.working_path.clone(),
        actor_integration_ref: binding_ref(resource, binding),
        actor_integration_source: resource.source.clone(),
        message_mode: binding.message_mode,
        integration_ref: integration_resource.effective_ref.clone(),
        integration_source: integration_resource.source.clone(),
        adapter_ref: adapter.effective_ref.clone(),
        adapter_name: if adapter.definition.name.is_empty() { adapter.effective_ref.clone() } else { adapter.definition.name.clone() },
        available_authority,
        missing_required_authority,
        // Inspection uses only the catalog projection; unresolved parameters never execute.
        adapter_params: adapter_params.value,
        integration_params: integration_params.value,
        actor_integration_params: binding_params.value,
        input_uses,
        input_plan: crate::input_delivery::InvocationInputs::default(),
    })
}

pub(crate) fn serialize_shell_environment_values(value: &mut Value) -> CwrResult<()> {
    let Some(environment) = value.get_mut("env").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    for value in environment.values_mut() {
        if value.is_object() || value.is_array() {
            *value = Value::String(serde_json::to_string(value)?);
        }
    }
    Ok(())
}

fn action_route(
    action: &ActionFile,
    role: &LoadedResource,
    binding: &ResolvedCwrActorIntegration,
    graph: &ResolvedGraph,
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<ResolvedCwrActionRoute> {
    let action_name = action.action_name();
    if binding.adapter_ref == crate::adapter::DECISIONS_ADAPTER_REF
        && binding.adapter_params.get("action_name").and_then(Value::as_str)
            != Some(action_name.as_str())
    {
        return invalid(format!(
            "{}: Decisions binding {} configures action {:?}, but role {} advertises {}; each advertised action must match the binding's configured action_name",
            role.source.display(), binding.actor_integration_ref,
            binding.adapter_params.get("action_name"), role.effective_ref, action_name,
        ));
    }
    let schema_root = Some(graph.scopes[role.scope_id].root_ref.as_str());
    Ok(ResolvedCwrActionRoute {
        role_ref: role.effective_ref.clone(),
        contract_revision: action.contract_revision.clone(),
        execution_scope: action.execution_scope,
        response_mode: action.response_mode,
        actor_ref: binding.actor_ref.clone(),
        coworker_ref: binding.coworker_ref.clone(),
        coworker_source: binding.coworker_source.clone(),
        working_path: binding.working_path.clone(),
        action_name,
        action_title: action.title.clone(),
        action_description: action.description.clone(),
        input_schema: load_schema_locator(
            &role.source,
            "action input schema",
            &action.input_schema,
            None,
            schema_root,
            resolver,
        )?,
        output_schema: load_schema_locator(
            &role.source,
            "action output schema",
            &action.output_schema,
            None,
            schema_root,
            resolver,
        )?,
        actor_integration_ref: binding.actor_integration_ref.clone(),
        actor_integration_source: binding.actor_integration_source.clone(),
        integration_ref: binding.integration_ref.clone(),
        integration_source: binding.integration_source.clone(),
        adapter_ref: binding.adapter_ref.clone(),
        available_authority: binding.available_authority.clone(),
        missing_required_authority: binding.missing_required_authority.clone(),
        adapter_params: binding.adapter_params.clone(),
        integration_params: binding.integration_params.clone(),
        actor_integration_params: binding.actor_integration_params.clone(),
        input_uses: binding.input_uses.clone(),
        input_plan: binding.input_plan.clone(),
    })
}

fn validate_role(
    resource: &LoadedResource,
    role: &RoleFile,
    graph: &ResolvedGraph,
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<()> {
    let mut adapters = BTreeSet::new();
    for binding in &role.adapters {
        let reference = binding
            .locator
            .reference
            .as_deref()
            .expect("resolved adapter relationship");
        let adapter = resolve_adapter(&resource.source, reference, graph, resource.scope_id)?;
        if !adapters.insert(adapter.effective_ref.clone()) {
            return invalid(format!(
                "{}: duplicate adapter {}",
                resource.source.display(),
                adapter.effective_ref
            ));
        }
        validate_adapter_params(
            &resource.source,
            "role adapter parameters",
            &adapter,
            adapter.definition.coworker_params_schema.as_ref(),
            &binding.params,
            resolver,
        )?;
    }
    let mut actions = BTreeSet::new();
    for action in &role.actions {
        if !actions.insert(action.action_name()) {
            return invalid(format!(
                "{}: duplicate action {}",
                resource.source.display(),
                action.action_name()
            ));
        }
        for (label, locator) in [
            ("action input schema", &action.input_schema),
            ("action output schema", &action.output_schema),
        ] {
            load_schema_locator(
                &resource.source,
                label,
                locator,
                None,
                Some(&graph.scopes[resource.scope_id].root_ref),
                resolver,
            )?;
        }
    }
    Ok(())
}

fn route_authority(
    required: &[String],
    actor: &[String],
    adapter: &[String],
) -> (Vec<String>, Vec<String>) {
    let actor = actor.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let adapter = adapter.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let available = actor
        .intersection(&adapter)
        .map(|effect| (*effect).to_owned())
        .collect::<Vec<_>>();
    let missing = required
        .iter()
        .filter(|effect| !available.contains(effect))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    (available, missing)
}

fn normalized_authority(authority: &[String]) -> Vec<String> {
    authority
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn resolve_native_tools(
    resource: &LoadedResource,
    coworker: &CoworkerFile,
    graph: &ResolvedGraph,
) -> CwrResult<Vec<ResolvedNativeTool>> {
    coworker
        .native_tools
        .iter()
        .map(|tool| {
            let requested =
                resolve_mount_ref(&graph.scopes[resource.scope_id].root_ref, &tool.coworker);
            // Native preferences express declared dependencies; canonical discovery remains runtime-wide.
            let target = graph.scopes[resource.scope_id]
                .lookup
                .get(&requested)
                .ok_or_else(|| {
                    invalid_error(format!(
                        "native tool coworker {} is not a declared dependency",
                        tool.coworker
                    ))
                })?;
            let ResolvedTarget::Resource(index) = target else {
                return invalid("native tool must select a coworker");
            };
            let target = &graph.resources[*index];
            let ResourceKind::Coworker(target_coworker) = &target.kind else {
                return invalid("native tool must select a coworker, not a role");
            };
            let roles = selected_roles(target, target_coworker, graph)?;
            if !roles.iter().any(|(_, role)| {
                role.actions
                    .iter()
                    .any(|action| action.action_name() == tool.action)
            }) {
                return invalid(format!(
                    "native tool selects unknown action {}",
                    tool.action
                ));
            }
            let binding_ref = if let Some(name) = &tool.binding {
                let binding = target_coworker
                    .integrations
                    .iter()
                    .find(|binding| binding.locator.alias.as_ref() == Some(name))
                    .ok_or_else(|| {
                        invalid_error(format!("native tool selects unknown binding {name}"))
                    })?;
                Some(binding_ref(target, binding))
            } else {
                default_binding(target, target_coworker)?
            };
            Ok(ResolvedNativeTool {
                coworker_ref: target.effective_ref.clone(),
                action_name: tool.action.clone(),
                binding_ref,
            })
        })
        .collect()
}

fn inferred_coworker_slug(coworker: &CoworkerFile) -> String {
    let name = coworker.name.as_deref().unwrap_or("coworker");
    let mut slug = name
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join("_");
    if !slug.starts_with(|character: char| character.is_ascii_lowercase()) {
        slug = format!("coworker_{slug}").trim_end_matches('_').to_owned();
    }
    slug
}

fn normalize_action_permissions(
    resource: &LoadedResource,
    params: &Parameters,
    graph: &ResolvedGraph,
) -> CwrResult<Parameters> {
    let original = params;
    let mut params = params.clone();
    if original.known_string_at("/cwr_action_actor_refs") {
        return invalid("cwr_action_actor_refs must be an array");
    }
    if original.unknown_at("/cwr_action_actor_refs") {
        return Ok(params);
    }
    if let Some(targets) = params.value.get_mut("cwr_action_actor_refs") {
        let targets = targets
            .as_array_mut()
            .ok_or_else(|| invalid_error("cwr_action_actor_refs must be an array"))?;
        for (index, target) in targets.iter_mut().enumerate() {
            if original.unknown_at(&format!("/cwr_action_actor_refs/{index}")) {
                continue;
            }
            let requested = target
                .as_str()
                .ok_or_else(|| invalid_error("cwr_action_actor_refs must contain references"))?;
            let reference = resolve_graph_ref(graph, resource.scope_id, requested);
            if reference != crate::action_contract::INTRINSIC_CWR_ACTOR_REF {
                resource_of_type(&resource.source, &reference, "permitted action target", graph, resource.scope_id, |kind| matches!(kind, ResourceKind::Coworker(_) | ResourceKind::Gateway(_)))?;
            }
            *target = Value::String(reference);
        }
    }
    Ok(params)
}
