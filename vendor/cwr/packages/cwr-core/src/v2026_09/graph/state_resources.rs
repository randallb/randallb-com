use super::*;

pub(super) fn validate_state_resources(
    graph: &ResolvedGraph,
    actors: &[ResolvedCwrActor],
    routes: &[ResolvedCwrActionRoute],
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<()> {
    let retained: BTreeSet<_> = actors.iter().map(|actor| actor.actor_ref.clone()).collect();
    let mut invocation_parents = BTreeMap::new();
    for resource in &graph.resources {
        match &resource.kind {
            ResourceKind::Conversation(conversation) => {
                let mut participants = BTreeSet::new();
                for participant_ref in &conversation.participant_actor_refs {
                    let participant_ref =
                        resolve_graph_ref(graph, resource.scope_id, participant_ref);
                    let participant = resource_of_type(
                        &resource.source,
                        &participant_ref,
                        "actor",
                        graph,
                        resource.scope_id,
                        |kind| matches!(kind, ResourceKind::Coworker(_)),
                    )?;
                    require_retained_state_actor(&participant.effective_ref, &retained)?;
                    if !participants.insert(participant.effective_ref.clone()) {
                        return invalid(format!(
                            "{}: conversation participant {} appears more than once after ref resolution",
                            resource.source.display(),
                            participant.effective_ref
                        ));
                    }
                }
            }
            ResourceKind::ActionInvocation(invocation) => {
                let parent = validate_action_invocation(
                    resource, invocation, graph, &retained, routes, resolver,
                )?;
                if let Some(parent) = parent {
                    invocation_parents.insert(resource.effective_ref.clone(), parent);
                }
            }
            _ => {}
        }
    }
    for invocation_ref in invocation_parents.keys() {
        let mut visited = BTreeSet::new();
        let mut current = invocation_ref.as_str();
        while let Some(parent) = invocation_parents.get(current) {
            if !visited.insert(current.to_owned()) {
                return invalid(format!("action invocation parent cycle contains {current}"));
            }
            current = parent;
        }
    }
    Ok(())
}

fn validate_action_invocation(
    resource: &LoadedResource,
    invocation: &ActionInvocationFile,
    graph: &ResolvedGraph,
    retained: &BTreeSet<String>,
    routes: &[ResolvedCwrActionRoute],
    _resolver: &dyn CheckResourceResolver,
) -> CwrResult<Option<String>> {
    let requester_ref =
        resolve_graph_ref(graph, resource.scope_id, &invocation.requester_actor_ref);
    resource_of_type(
        &resource.source,
        &requester_ref,
        "requester actor",
        graph,
        resource.scope_id,
        |kind| matches!(kind, ResourceKind::Coworker(_)),
    )?;
    require_retained_state_actor(&requester_ref, retained)?;
    let target_ref = resolve_graph_ref(graph, resource.scope_id, &invocation.target_actor_ref);
    let target = resource_of_type(
        &resource.source,
        &target_ref,
        "target actor",
        graph,
        resource.scope_id,
        |kind| matches!(kind, ResourceKind::Coworker(_)),
    )?;
    require_retained_state_actor(&target.effective_ref, retained)?;
    let route = routes
        .iter()
        .find(|route| {
            route.actor_ref == target.effective_ref
                && route.action_name == invocation.action_name
                && invocation
                    .actor_integration_ref
                    .as_ref()
                    .is_none_or(|binding| &route.actor_integration_ref == binding)
        })
        .ok_or_else(|| invalid_error("retained invocation has no exact current action route"))?;
    route.validate_input(&invocation.input)?;
    if let Some(conversation_ref) = &invocation.conversation_ref {
        let conversation_ref = resolve_graph_ref(graph, resource.scope_id, conversation_ref);
        resource_of_type(
            &resource.source,
            &conversation_ref,
            "conversation",
            graph,
            resource.scope_id,
            |kind| matches!(kind, ResourceKind::Conversation(_)),
        )?;
    }
    if let Some(stop_actor_ref) = &invocation.stop_requested_by_actor_ref {
        let stop_actor_ref = resolve_graph_ref(graph, resource.scope_id, stop_actor_ref);
        require_retained_state_actor(&stop_actor_ref, retained)?;
        resource_of_type(
            &resource.source,
            &stop_actor_ref,
            "stop-requesting actor",
            graph,
            resource.scope_id,
            |kind| matches!(kind, ResourceKind::Coworker(_)),
        )?;
    }
    let mut previous_sequence = 0;
    let mut terminal_index = None;
    for (index, response) in invocation.responses.iter().enumerate() {
        let response_invocation_ref =
            resolve_graph_ref(graph, resource.scope_id, &response.invocation_ref);
        if response_invocation_ref != resource.effective_ref {
            return invalid(format!(
                "{}: response {} names invocation {}, not {}",
                resource.source.display(),
                response.sequence,
                response_invocation_ref,
                resource.effective_ref
            ));
        }
        if response.sequence <= previous_sequence {
            return invalid(format!(
                "{}: response sequence numbers must be strictly increasing",
                resource.source.display()
            ));
        }
        previous_sequence = response.sequence;
        if response.status.is_terminal() {
            terminal_index = Some(index);
        }
        if response.status.is_completed() {
            let result = response.result.as_ref().ok_or_else(|| {
                invalid_error(format!(
                    "{}: completed response is missing result",
                    resource.source.display()
                ))
            })?;
            let validator = json_schema_validator(&route.output_schema)
                .map_err(|error| invalid_error(error.to_string()))?;
            if !validator.is_valid(result) {
                return invalid("retained action result does not satisfy its output schema");
            }
        }
    }
    if terminal_index.is_some_and(|index| index + 1 != invocation.responses.len()) {
        return invalid(format!(
            "{}: the terminal response must be last",
            resource.source.display()
        ));
    }

    let parent = if let Some(parent_ref) = &invocation.parent_invocation_ref {
        let parent_ref = resolve_graph_ref(graph, resource.scope_id, parent_ref);
        let parent = resource_of_type(
            &resource.source,
            &parent_ref,
            "parent invocation",
            graph,
            resource.scope_id,
            |kind| matches!(kind, ResourceKind::ActionInvocation(_)),
        )?;
        Some(parent.effective_ref.clone())
    } else {
        None
    };
    Ok(parent)
}

fn require_retained_state_actor(reference: &str, retained: &BTreeSet<String>) -> CwrResult<()> {
    if !retained.contains(reference) {
        return invalid(format!(
            "shared state names unavailable coworker {reference}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    #[test]
    fn retained_graph_responses_classify_noncanonical_codes_and_preserve_null_results() {
        let progress: ActionResponseFile = serde_json::from_value(json!({
            "invocation_ref": "~@/invocations/one",
            "sequence": 1,
            "status": 150
        }))
        .unwrap();
        assert!(progress.status.is_progress());
        assert!(!progress.status.is_terminal());
        assert!(progress.result.is_none());

        let completed: ActionResponseFile = serde_json::from_value(json!({
            "invocation_ref": "~@/invocations/one",
            "sequence": 2,
            "status": 202,
            "result": null
        }))
        .unwrap();
        assert!(completed.status.is_completed());
        assert!(completed.status.is_terminal());
        assert_eq!(completed.result, Some(Value::Null));

        assert!(serde_json::from_value::<ActionResponseFile>(json!({
            "invocation_ref": "~@/invocations/one",
            "sequence": 1,
            "status": 300
        }))
        .is_err());
    }
}
