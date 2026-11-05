/// Inspect a checked CWR graph without choosing an acting actor or opening
/// runtime-owned conversation state.
pub fn inspect_host_workspace(graph: &ResolvedCwrGraph) -> CwrResult<HostWorkspace> {
    let actors = graph
        .actors
        .iter()
        .map(crate::v2026_09::local::local_actor_identity)
        .collect::<CwrResult<Vec<_>>>()?
        .iter()
        .map(project_host_actor)
        .collect();
    let workspace = HostWorkspace {
        workspace_ref: graph.reference.clone(),
        runtime_ref: graph.runtime_ref.clone(),
        actors,
        warnings: graph.warnings.clone(),
    };
    validate_workspace(&workspace)?;
    Ok(workspace)
}

/// Resolve and inspect a local CWR workspace without selecting an acting actor.
pub fn inspect_local_workspace(input: impl AsRef<Path>) -> CwrResult<HostWorkspace> {
    inspect_host_workspace(&crate::v2026_09::local::resolve_local_graph(input.as_ref())?)
}

/// Project a graph actor identity onto the allowlisted host wire fields.
pub fn project_host_actor(actor: &LocalActorIdentity) -> HostActor {
    HostActor {
        actor_ref: actor.actor_ref.clone(),
        coworker_ref: actor.coworker_ref.clone(),
        handle: actor.handle.clone(),
        name: actor.name.clone(),
        category: actor.category,
        provider_model: actor.provider_model.clone(),
    }
}

/// Project the selected local catalog without exposing adapter or integration
/// configuration to the presentation host.
pub fn project_host_catalog(catalog: &LocalCatalog) -> HostActionCatalog {
    HostActionCatalog {
        known_actors: catalog.known_actors.iter().map(project_host_actor).collect(),
        retained_actors: catalog.retained_actors.iter().map(project_host_actor).collect(),
        default_conversation_target_ref: catalog.default_conversation_target_ref.clone(),
        actors: catalog.actors.iter().map(project_catalog_actor).collect(),
        actions: catalog
            .actions
            .iter()
            .map(|action| HostAction {
                activation: action.activation.clone(),
                actor_ref: action.actor_ref.clone(),
                action_name: action.action_name.clone(),
                title: action.title.clone(),
                description: action.description.clone(),
                input_schema: action.input_schema.clone(),
                output_schema: action.output_schema.clone(),
            })
            .collect(),
    }
}

fn project_catalog_actor(actor: &LocalActor) -> HostActor {
    HostActor {
        actor_ref: actor.actor_ref.clone(),
        coworker_ref: actor.coworker_ref.clone(),
        handle: actor.handle.clone(),
        name: actor.name.clone(),
        category: actor.category,
        provider_model: actor.provider_model.clone(),
    }
}

fn validate_workspace(workspace: &HostWorkspace) -> CwrResult<()> {
    validate_ref("workspace", &workspace.workspace_ref)?;
    validate_ref("configured runtime", &workspace.runtime_ref)?;
    validate_actors("workspace", &workspace.actors)
}

fn validate_action_catalog(catalog: &HostActionCatalog) -> CwrResult<()> {
    if let Some(reference) = &catalog.default_conversation_target_ref {
        validate_ref("default conversation target", reference)?;
    }
    validate_actors("known host actors", &catalog.known_actors)?;
    validate_actors("host action catalog", &catalog.actors)?;
    let actor_refs = catalog
        .actors
        .iter()
        .map(|actor| actor.actor_ref.as_str())
        .collect::<BTreeSet<_>>();
    let mut activations = BTreeSet::new();
    for action in &catalog.actions {
        require_nonblank("host action activation", &action.activation)?;
        validate_ref("host action actor", &action.actor_ref)?;
        if !actor_refs.contains(action.actor_ref.as_str()) {
            return invalid(format!(
                "host action {} refers to actor {} outside its catalog",
                action.activation, action.actor_ref
            ));
        }
        require_nonblank("host action name", &action.action_name)?;
        require_nonblank("host action title", &action.title)?;
        require_nonblank("host action description", &action.description)?;
        if !activations.insert(&action.activation) {
            return invalid(format!(
                "host action catalog repeats activation {}",
                action.activation
            ));
        }
    }
    Ok(())
}

fn validate_actors(label: &str, actors: &[HostActor]) -> CwrResult<()> {
    let mut actor_refs = BTreeSet::new();
    let mut handles = BTreeSet::new();
    for actor in actors {
        validate_actor(actor)?;
        if !actor_refs.insert(&actor.actor_ref) {
            return invalid(format!("{label} repeats actor {}", actor.actor_ref));
        }
        if !handles.insert(&actor.handle) {
            return invalid(format!("{label} repeats actor handle {}", actor.handle));
        }
    }
    Ok(())
}

fn validate_actor(actor: &HostActor) -> CwrResult<()> {
    validate_ref("host actor", &actor.actor_ref)?;
    validate_ref("host actor coworker", &actor.coworker_ref)?;
    require_nonblank("host actor handle", &actor.handle)?;
    if !actor.handle.starts_with('@') || actor.handle.len() == 1 {
        return invalid(format!(
            "host actor handle {} must begin with @ and contain a name",
            actor.handle
        ));
    }
    require_nonblank("host actor name", &actor.name)
}
