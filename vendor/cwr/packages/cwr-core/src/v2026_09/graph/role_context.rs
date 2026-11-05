fn compose_adapter_params(
    resource: &LoadedResource,
    binding: &IntegrationBinding,
    roles: &[(&LoadedResource, &RoleFile)],
    adapter: &ResolvedAdapter,
    graph: &ResolvedGraph,
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<Parameters> {
    let mut deferred_conflict = false;
    let mut unresolved = BTreeMap::new();
    let mut input_sources = BTreeMap::new();
    let mut shared = serde_json::Map::new();
    let mut origins = BTreeMap::new();
    let mut contexts = Vec::new();
    let mut sources = serde_json::Map::new();
    let mut entrypoints = Vec::new();
    for (role_resource, role) in roles {
        for locator in &role.instructions {
            let path = instruction_file(role_resource, locator, graph, resolver)?;
            let canonical = path.to_string_lossy().into_owned();
            sources.insert(canonical.clone(), Value::String(fs::read_to_string(&path)?));
            entrypoints.push(Value::String(canonical));
        }
        let selected = role
            .adapters
            .iter()
            .find(|binding| binding.locator.reference.as_deref() == Some(&adapter.effective_ref))
            .ok_or_else(|| {
                invalid_error(format!(
                    "binding {} does not support selected role {} through adapter {}",
                    binding_ref(resource, binding),
                    role_resource.effective_ref,
                    adapter.effective_ref
                ))
            })?;
        if role
            .adapters
            .iter()
            .filter(|candidate| {
                candidate.locator.reference.as_deref() == Some(&adapter.effective_ref)
            })
            .count()
            != 1
        {
            return invalid("duplicate role adapter configuration");
        }
        validate_adapter_params(
            &role_resource.source,
            "role adapter parameters",
            adapter,
            adapter.definition.coworker_params_schema.as_ref(),
            &selected.params,
            resolver,
        )?;
        let selected_params = &selected.params;
        input_sources.extend(selected_params.sources.clone());
        if !selected_params.can_be_object() {
            return invalid("role adapter params must be an object");
        }
        if selected_params.unknown_at("") {
            if !shared.is_empty()
                || (!unresolved.is_empty() && unresolved != selected_params.unresolved)
            {
                deferred_conflict = true;
            }
            unresolved.extend(selected_params.unresolved.clone());
            continue;
        }
        let params = selected_params
            .value
            .as_object()
            .ok_or_else(|| invalid_error("role adapter params must be an object"))?;
        for (key, value) in params {
            if key == "entrypoint" {
                // Role entrypoints are independent sources, not shared settings.
                if !selected_params.unknown_at("/entrypoint") {
                    let path = value
                        .as_str()
                        .ok_or_else(|| invalid_error("role entrypoint must be a path"))?;
                    let path =
                        resolve_regular_file(&role_resource.source, path, "role entrypoint")?;
                    resolver::source_file(&path, &graph.source_roots)?;
                    let canonical = path.to_string_lossy().into_owned();
                    sources.insert(canonical.clone(), Value::String(fs::read_to_string(&path)?));
                    entrypoints.push(Value::String(canonical));
                }
                shared.entry(key.clone()).or_insert_with(|| value.clone());
                continue;
            }
            let previous = Parameters {
                value: Value::Null,
                unresolved: unresolved.clone(),
                sources: input_sources.clone(),
            };
            if let Some(existing) = shared.get(key) {
                match previous.equivalent_value_at(
                    &parameters::child_pointer("", key),
                    existing,
                    selected_params,
                    value,
                ) {
                    requirements::InputEquivalence::Conflicting => {
                        return invalid(format!(
                            "binding {} has conflicting shared parameter {key} from roles {} and {}",
                            binding_ref(resource, binding),
                            origins[key],
                            role_resource.effective_ref
                        ));
                    }
                    requirements::InputEquivalence::Unresolved => deferred_conflict = true,
                    requirements::InputEquivalence::Equal => {}
                }
            } else if previous.unknown_at("") {
                deferred_conflict = true;
            }
            shared.insert(key.clone(), value.clone());
            origins.insert(key.clone(), role_resource.effective_ref.clone());
        }
        unresolved.extend(selected_params.unresolved.clone());
        contexts.push(serde_json::json!({"role_ref": role_resource.effective_ref, "source": role_resource.source, "params": selected.params}));
    }
    // Import edges determine ownership even when a coworker lives outside the
    // runtime's directory. Ambiguous shared coworkers are rejected at load.
    if let Some(volumes) = graph.coworker_volumes.get(&resource.source) {
        let entrypoint = format!("{}#local-volumes", resource.source.display());
        let mut instructions = String::from("Locally bound working folders for this handler:\n");
        for (alias, (path, access)) in volumes {
            let quoted_path = serde_json::to_string(&path.to_string_lossy())?;
            instructions.push_str(&format!("- {alias} ({access}): {quoted_path}\n"));
        }
        sources.insert(entrypoint.clone(), Value::String(instructions));
        entrypoints.push(Value::String(entrypoint));
    }
    // A binding is not an override for conflicting role execution settings.
    if let Some(params) = binding.params.value.as_object() {
        for (key, value) in params {
            if key == "entrypoint" {
                continue;
            }
            let previous = Parameters {
                value: Value::Null,
                unresolved: unresolved.clone(),
                sources: input_sources.clone(),
            };
            if let Some(shared) = shared.get(key) {
                match previous.equivalent_value_at(
                    &parameters::child_pointer("", key),
                    shared,
                    &binding.params,
                    value,
                ) {
                    requirements::InputEquivalence::Conflicting => {
                        return invalid(format!(
                            "binding {} conflicts with role parameter {key}",
                            binding_ref(resource, binding)
                        ));
                    }
                    requirements::InputEquivalence::Unresolved => deferred_conflict = true,
                    requirements::InputEquivalence::Equal => {}
                }
            } else if previous.unknown_at("") {
                deferred_conflict = true;
            }
        }
    } else if binding.params.unknown_at("") && !shared.is_empty() {
        deferred_conflict = true;
    }
    let mut composed = Parameters {
        value: Value::Object(shared),
        unresolved,
        sources: input_sources,
    };
    validate_adapter_params(
        &resource.source,
        "composed coworker parameters",
        adapter,
        adapter.definition.coworker_params_schema.as_ref(),
        &composed,
        resolver,
    )?;
    if deferred_conflict {
        return Err(Box::new(parameters::UnresolvedParameterConflict));
    }
    let shared = composed
        .value
        .as_object_mut()
        .expect("composed parameters are an object");
    shared.insert("role_contexts".into(), Value::Array(contexts));
    shared.insert("activated_sources".into(), Value::Object(sources));
    shared.insert("activated_entrypoints".into(), Value::Array(entrypoints));
    Ok(composed)
}

fn instruction_file(
    role: &LoadedResource,
    locator: &SchemaLocator,
    graph: &ResolvedGraph,
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<PathBuf> {
    let path = match (&locator.path, &locator.reference) {
        (Some(path), None) => {
            if !(path.starts_with("./") || path.starts_with("../")) {
                return invalid("instruction path must start with ./ or ../");
            }
            role.source.parent().expect("role source parent").join(path)
        }
        (None, Some(reference)) => {
            let reference = resolve_mount_ref(&role.effective_ref, reference);
            resolver
                .resolve(&reference)?
                .ok_or_else(|| invalid_error(format!("unresolved instruction ref {reference}")))?
        }
        _ => return invalid("instruction locator must contain exactly one of path or ref"),
    };
    resolver::source_file(&path, &graph.source_roots)
}
