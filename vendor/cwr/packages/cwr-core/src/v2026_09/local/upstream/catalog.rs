fn upstream_graph(
    state: &LocalRuntimeState,
    owner: Option<&str>,
) -> CwrResult<ResolvedCwrGraph> {
    let mut graph = state.graph.clone();
    for route in state.upstream.visible(owner)? {
        let declaration = &route.coworker;
        let source = state.cwr_toml.clone();
        let integration = format!(
            "{}/routes/{}",
            route.provider_session_ref,
            declaration.handle.trim_start_matches('@')
        );
        let params = json!({"upstream_route":route});
        graph.resources.push(ResolvedCwrResource {
            source: source.clone(), scope_ref: route.provider_session_ref.clone(),
            type_ref: "cwp://org.coworkerprotocol/2026-09/schemas/actor".into(), reference: declaration.actor_ref.clone(),
            visibility: ResolvedCwrVisibility::Import,
            document: json!({"ref":declaration.actor_ref, "coworker_ref":declaration.coworker_ref, "name":declaration.name}),
        });
        graph.actors.push(ResolvedCwrActor {
            default_conversation_target_ref: None,
            native_tools: Vec::new(),
            role_refs: vec![declaration.coworker_ref.clone()],
            message_binding_preference: Vec::new(),
        default_actor_integration_ref: Some(integration.clone()),
            actor_ref: declaration.actor_ref.clone(),
            actor_source: source.clone(),
            actor_name: Some(declaration.name.clone()),
            authority: Vec::new(),
            coworker_ref: declaration.coworker_ref.clone(),
            coworker_source: source.clone(),
            coworker_name: Some(declaration.name.clone()),
            coworker_slug: declaration.handle.trim_start_matches('@').into(),
            category: None,
            provider_model: None,
        });
        graph.actor_integrations.push(ResolvedCwrActorIntegration {
            actor_ref: declaration.actor_ref.clone(),
            actor_name: Some(declaration.name.clone()),
            coworker_ref: declaration.coworker_ref.clone(),
            coworker_source: source.clone(),
            working_path: source.parent().unwrap().into(),
            actor_integration_ref: integration.clone(),
            actor_integration_source: source.clone(),
            message_mode: Default::default(),
            integration_ref: route.provider_session_ref.clone(),
            integration_source: source.clone(),
            adapter_ref: UPSTREAM_ADAPTER_REF.into(),
            adapter_name: "Upstream".into(),
            available_authority: Vec::new(),
            missing_required_authority: Vec::new(),
            adapter_params: params.clone(),
            integration_params: json!({}),
            actor_integration_params: json!({}),
            input_uses: Vec::new(),
            input_plan: Default::default(),
        });
        for action in &declaration.actions {
            graph.routes.push(ResolvedCwrActionRoute {
                role_ref: declaration.coworker_ref.clone(),
                contract_revision: "1".into(),
                execution_scope: crate::action_contract::ExecutionScope::RequestingConversation,
                response_mode: crate::action_contract::ResponseMode::default(),
                actor_ref: declaration.actor_ref.clone(),
                coworker_ref: declaration.coworker_ref.clone(),
                coworker_source: source.clone(),
                working_path: source.parent().unwrap().into(),
                action_name: action.action_name.clone(),
                action_title: action.title.clone(),
                action_description: action.description.clone(),
                input_schema: action.input_schema.clone(),
                output_schema: action.output_schema.clone(),
                actor_integration_ref: integration.clone(),
                actor_integration_source: source.clone(),
                integration_ref: route.provider_session_ref.clone(),
                integration_source: source.clone(),
                adapter_ref: UPSTREAM_ADAPTER_REF.into(),
                available_authority: Vec::new(),
                missing_required_authority: Vec::new(),
                adapter_params: params.clone(),
                integration_params: json!({}),
                actor_integration_params: json!({}),
            input_uses: Vec::new(),
            input_plan: Default::default(),
            });
        }
    }
    Ok(graph)
}

fn runtime_actor_identity(
    graph: &ResolvedCwrGraph,
    actor: &ResolvedCwrActor,
) -> CwrResult<LocalActorIdentity> {
    let mut identity = local_actor_identity(actor)?;
    if let Some(binding) = graph.actor_integrations.iter().find(|binding| {
        binding.actor_ref == actor.actor_ref && binding.adapter_ref == UPSTREAM_ADAPTER_REF
    }) {
        let route: UpstreamRoute =
            serde_json::from_value(binding.adapter_params["upstream_route"].clone())?;
        identity.handle = route.coworker.handle;
    }
    Ok(identity)
}
