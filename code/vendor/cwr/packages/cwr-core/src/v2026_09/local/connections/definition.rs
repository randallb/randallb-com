use super::*;

pub(super) struct Definition {
    pub path: PathBuf,
    pub fingerprint: String,
    pub requirements: Vec<InputDeclaration>,
    pub graph: ResolvedCwrGraph,
}

#[cfg(test)]
pub(super) fn definition(path: &Path) -> CwrResult<Definition> {
    definition_for_target(&connection_target(path)?)
}

pub(super) fn definition_for_target(target: &ConnectionTarget) -> CwrResult<Definition> {
    let requirements = graph::requirements::inspect_input_declarations(&target.path)?;
    let mut options = target.options.clone();
    options.input_resolution = Default::default();
    options.input_resolution.enforce_destinations = true;
    let inputs = options
        .inputs
        .as_object_mut()
        .ok_or("Expected configured inputs.")?;
    for declaration in &requirements {
        if workspace_kind(declaration).is_some() || !inputs.contains_key(&declaration.alias) {
            inputs.insert(
                declaration.alias.clone(),
                json!(format!("inspection-{}", declaration.alias)),
            );
        }
    }
    // Provenance, not these dummy values, defines supported destinations.
    let graph = match graph::resolve_bound_graph(
        &target.path,
        &options,
        graph::requirements::concrete_bindings(&options.inputs)?,
    ) {
        Ok(graph) if graph.warnings.is_empty() => graph,
        result => {
            let fallback = result;
            // Missing ordinary JSON can have any shape. Inspect its provenance
            // without pretending a string is valid JSON for its receiver.
            let mut inspection = options.clone();
            for declaration in &requirements {
                if workspace_kind(declaration).is_none()
                    && target.options.inputs.get(&declaration.alias).is_none()
                {
                    inspection
                        .inputs
                        .as_object_mut()
                        .expect("object inputs")
                        .remove(&declaration.alias);
                }
            }
            let mut bindings = graph::requirements::concrete_bindings(&inspection.inputs)?;
            for declaration in &requirements {
                if workspace_kind(declaration).is_none()
                    && target.options.inputs.get(&declaration.alias).is_none()
                {
                    bindings.insert(
                        declaration.alias.clone(),
                        graph::requirements::InputBinding::missing(),
                    );
                }
            }
            let (_, graph) = graph::inspect_partial_bound_graph(
                &target.path,
                &graph::EmptyResourceResolver,
                &inspection,
                bindings,
            )?;
            match graph {
                Some(graph) => graph,
                None => fallback?,
            }
        }
    };
    let mut wire = serde_json::to_value(&graph)?;
    let fields = wire.as_object_mut().ok_or("Expected graph.")?;
    let runtime = fields
        .remove("runtime_ref")
        .ok_or("Expected runtime identity.")?;
    fields.insert("configured_loop_ref".into(), runtime);
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&wire)?);
    let mut sources: BTreeSet<_> = graph.resources.iter().map(|r| r.source.clone()).collect();
    sources.insert(target.path.clone());
    for source in sources {
        digest.update(fs::read(source)?);
    }
    Ok(Definition {
        path: target.path.clone(),
        fingerprint: hex::encode(digest.finalize()),
        requirements,
        graph,
    })
}

pub(super) fn workspace_kind(declaration: &InputDeclaration) -> Option<EnvironmentEntryKind> {
    if declaration.type_ref == SECRET_TYPE {
        Some(EnvironmentEntryKind::Secret)
    } else if declaration.storage == Some(PlaintextStorage::WorkspaceVariable) {
        Some(EnvironmentEntryKind::Variable)
    } else {
        None
    }
}
