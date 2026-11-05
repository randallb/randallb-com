use super::*;

pub(super) struct Resolution {
    pub options: graph::GraphLoadOptions,
    pub rows: Vec<ProjectConnection>,
}

pub(super) fn resolve(
    target: &ConnectionTarget,
    definition: &Definition,
    grants: &Grants,
    selected: Option<&BTreeSet<String>>,
) -> CwrResult<Resolution> {
    let current = |r: &InputDeclaration| {
        grants.connections.get(&r.alias).filter(|g| {
            g.fingerprint == definition.fingerprint
                && workspace_kind(r).is_some()
                && (g.backend == Backend::Workspace || r.type_ref == SECRET_TYPE)
        })
    };
    // Only effective selections decide which stores to open. Failure is attached
    // to that input; an unrelated binding remains executable.
    let selected_input =
        |r: &InputDeclaration| selected.is_none_or(|names| names.contains(&r.alias));
    let environment = definition
        .requirements
        .iter()
        .filter(|r| selected_input(r))
        .any(|r| current(r).is_some_and(|g| g.backend == Backend::Workspace))
        .then(|| EnvironmentAccess::open(&target.workspace, || false));
    let legacy = definition
        .requirements
        .iter()
        .filter(|r| selected_input(r))
        .any(|r| current(r).is_some_and(|g| g.backend == Backend::Legacy))
        .then(|| LegacyAccess::open(|| false));
    let mut options = target.options.clone();
    options.input_resolution = InputResolution {
        enforce_destinations: true,
        context: Some(InputContext {
            path: definition.path.clone(),
            workspace: target.workspace.clone(),
            grants_path: target.grants_path.clone(),
            runtime_ref: options.runtime_ref.clone(),
            storage_identity: crate::workspace_environment::storage_identity()?,
            definition: definition.fingerprint.clone(),
        }),
        selections: BTreeMap::new(),
    };
    let inputs = options
        .inputs
        .as_object_mut()
        .ok_or("Expected configured inputs.")?;
    let mut rows = Vec::new();
    for r in &definition.requirements {
        let grant = current(r);
        let local = grants
            .plaintext
            .get(&r.alias)
            .filter(|g| g.fingerprint == definition.fingerprint && workspace_kind(r).is_none());
        let mut row = ProjectConnection {
            alias: r.alias.clone(),
            type_ref: r.type_ref.clone(),
            purpose: r.purpose.clone(),
            required: r.required,
            key_id: grant.map(|g| g.key_id.clone()),
            approved: false,
            configured: false,
            local_value: None,
            destinations: destinations(definition, &r.alias),
            plaintext_storage: (r.type_ref == PLAINTEXT_TYPE)
                .then_some(r.storage.unwrap_or(PlaintextStorage::LocalJson)),
        };
        if let Some(grant) = grant {
            let selection = match grant.backend {
                Backend::Workspace => environment
                    .as_ref()
                    .and_then(|r| r.as_ref().ok())
                    .and_then(|db| db.metadata(&grant.key_id).ok())
                    .filter(|m| Some(m.kind) == workspace_kind(r))
                    .map(|m| InputSelection::Workspace {
                        reference: grant.key_id.clone(),
                        revision: m.revision,
                        entry_kind: m.kind,
                        generation: grant.token.clone(),
                    }),
                Backend::Legacy => legacy
                    .as_ref()
                    .and_then(|r| r.as_ref().ok())
                    .filter(|keys| keys.contains(&grant.key_id))
                    .map(|_| InputSelection::Legacy {
                        key_id: grant.key_id.clone(),
                        generation: grant.token.clone(),
                    }),
            };
            row.approved = selection.is_some();
            row.configured = row.approved;
            options.input_resolution.selections.insert(
                r.alias.clone(),
                selection.unwrap_or(InputSelection::Unavailable),
            );
            inputs.insert(r.alias.clone(), json!(format!("managed-input:{}", r.alias)));
        } else if let Some(local) = local {
            row.configured = true;
            row.local_value = Some(local.value.clone());
            inputs.insert(r.alias.clone(), local.value.clone());
        } else if r.storage != Some(PlaintextStorage::WorkspaceVariable)
            && inputs.contains_key(&r.alias)
        {
            row.configured = true;
        } else if r.required || workspace_kind(r).is_some() {
            options.input_resolution.selections.insert(
                r.alias.clone(),
                if r.required {
                    InputSelection::Unavailable
                } else {
                    InputSelection::Absent
                },
            );
            inputs.insert(r.alias.clone(), json!(format!("managed-input:{}", r.alias)));
        }
        rows.push(row);
    }
    Ok(Resolution { options, rows })
}

// Optional picker refresh never changes the committed result or input status.
pub(super) fn report(
    target: &ConnectionTarget,
    definition: &Definition,
    grants: &Grants,
    rows: Vec<ProjectConnection>,
) -> ProjectConnections {
    let needs_picker = definition
        .requirements
        .iter()
        .any(|r| workspace_kind(r).is_some());
    let entries = if needs_picker {
        crate::workspace_environment::entries(&target.workspace)
    } else {
        Ok(vec![])
    };
    ProjectConnections {
        warnings: definition.graph.warnings.clone(),
        fingerprint: definition.fingerprint.clone(),
        grants_revision: grants_revision(grants).expect("serializable grants"),
        requirements: rows,
        environment_unavailable: entries.is_err(),
        environment_entries: entries.unwrap_or_default(),
    }
}

pub(crate) fn capture_for_binding(
    template: &crate::input_delivery::InvocationInputs,
    uses: &[crate::input_delivery::InputUse],
) -> CwrResult<crate::input_delivery::InvocationInputs> {
    if uses.is_empty() {
        return Ok(template.clone());
    }
    let context = template
        .context
        .as_ref()
        .ok_or("Input approval is incomplete.")?;
    let target = target_for_context(context)?;
    let definition = definition_for_target(&target)?;
    if definition.fingerprint != context.definition {
        return invalid("Input definition changed. Reopen this workspace.");
    }
    let grants = read_grants(&target.grants_path)?;
    let aliases = uses
        .iter()
        .filter(|u| u.source.scope == context.path.to_string_lossy())
        .map(|u| u.source.alias.clone())
        .collect();
    let options = resolve(&target, &definition, &grants, Some(&aliases))?.options;
    Ok(options
        .input_resolution
        .project(&template.binding_ref, uses))
}

fn destinations(definition: &Definition, alias: &str) -> Vec<String> {
    let ordinary = definition
        .graph
        .actor_integrations
        .iter()
        .flat_map(|binding| {
            binding
                .input_uses
                .iter()
                .filter(|usage| {
                    usage.source.alias == alias
                        && usage.source.scope == definition.path.to_string_lossy()
                })
                .map(|usage| {
                    let field = match &usage.destination {
                        crate::input_delivery::InputDestination::ShellEnv { name } => {
                            format!("environment {name}")
                        }
                        crate::input_delivery::InputDestination::OpenResponsesApiKey => {
                            "API key".into()
                        }
                    };
                    format!(
                        "{} / {}: {field}",
                        binding.actor_name.as_deref().unwrap_or("Integration"),
                        binding
                            .actor_integration_ref
                            .rsplit('/')
                            .next()
                            .unwrap_or("connection")
                    )
                })
        })
        .collect::<Vec<_>>();
    let gateways = definition.graph.gateways.iter().flat_map(|gateway| {
        [
            Some(&gateway.route),
            gateway.ingress_route.as_ref(),
            gateway.delivery_route.as_ref(),
        ]
        .into_iter()
        .flatten()
        .flat_map(move |route| {
            route
                .input_uses
                .iter()
                .filter(move |usage| {
                    usage.source.alias == alias
                        && usage.source.scope == definition.path.to_string_lossy()
                })
                .map(move |usage| {
                    let field = match &usage.destination {
                        crate::input_delivery::InputDestination::ShellEnv { name } => {
                            format!("environment {name}")
                        }
                        crate::input_delivery::InputDestination::OpenResponsesApiKey => {
                            "API key".into()
                        }
                    };
                    format!(
                        "Gateway {} / {}: {field}",
                        gateway.resource_ref, route.action_name
                    )
                })
        })
    });
    ordinary
        .into_iter()
        .chain(gateways)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
