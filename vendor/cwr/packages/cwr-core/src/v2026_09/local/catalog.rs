#[cfg(test)]
fn take_ordered_runnable_refs(
    state: &mut LocalRuntimeState,
    extra_refs: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let mut refs = std::mem::take(&mut state.pending_invocation_refs)
        .into_iter()
        .collect::<BTreeSet<_>>();
    refs.extend(state.admission.runnable_invocation_refs());
    refs.extend(extra_refs);
    order_runnable_refs(&state.admission, refs)
}

fn independently_scheduled(
    admission: &ConversationAdmissionStore,
    record: &ActionInvocationRecord,
) -> bool {
    admission
        .hydrated_execution_conversation(&record.invocation_ref)
        .is_some_and(|execution| {
            execution.activity.iter().any(|activity| {
                matches!(
                    &activity.event,
                    ConversationActivityEvent::TurnStarted {
                        invocation: Some(snapshot),
                        ..
                    } if snapshot.get("response_mode").and_then(Value::as_str) == Some("async")
                        && snapshot.get("execution_scope").and_then(Value::as_str)
                            == Some("own_conversation")
                )
            })
        })
}

fn execution_group_ref<'a>(
    admission: &'a ConversationAdmissionStore,
    record: &'a ActionInvocationRecord,
) -> Option<&'a str> {
    let mut current = record;
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(current.invocation_ref.as_str()) {
            return None;
        }
        if independently_scheduled(admission, current) {
            return admission.execution_conversation_ref(&current.invocation_ref);
        }
        let Some(parent_ref) = current.parent_invocation_ref.as_deref() else {
            return current.conversation_ref.as_deref();
        };
        current = admission.invocation_record(parent_ref)?;
    }
}

fn belongs_to_independently_scheduled_group(
    admission: &ConversationAdmissionStore,
    record: &ActionInvocationRecord,
) -> bool {
    let mut current = record;
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(current.invocation_ref.as_str()) {
            return false;
        }
        if independently_scheduled(admission, current) {
            return true;
        }
        let Some(parent_ref) = current.parent_invocation_ref.as_deref() else {
            return false;
        };
        let Some(parent) = admission.invocation_record(parent_ref) else {
            return false;
        };
        current = parent;
    }
}

fn take_ordered_runnable_refs_for_group(
    state: &mut LocalRuntimeState,
    group_ref: &str,
) -> Vec<String> {
    let refs = state
        .admission
        .runnable_invocation_refs()
        .into_iter()
        .filter(|invocation_ref| {
            state
                .admission
                .invocation_record(invocation_ref)
                .and_then(|record| execution_group_ref(&state.admission, record))
                == Some(group_ref)
        })
        .collect();
    order_runnable_refs(&state.admission, refs)
}

fn take_ordered_pending_and_extra_refs_for_group(
    state: &mut LocalRuntimeState,
    group_ref: &str,
    extra_refs: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let mut refs = take_ordered_pending_refs_for_group(state, group_ref)
        .into_iter()
        .collect::<BTreeSet<_>>();
    refs.extend(extra_refs);
    order_runnable_refs(&state.admission, refs)
}

fn take_ordered_pending_refs_for_group(
    state: &mut LocalRuntimeState,
    group_ref: &str,
) -> Vec<String> {
    let pending = std::mem::take(&mut state.pending_invocation_refs);
    let (matching, remaining): (Vec<_>, Vec<_>) = pending.into_iter().partition(|invocation_ref| {
        state
            .admission
            .invocation_record(invocation_ref)
            .and_then(|record| execution_group_ref(&state.admission, record))
            == Some(group_ref)
    });
    state.pending_invocation_refs = remaining;
    order_runnable_refs(&state.admission, matching.into_iter().collect())
}

fn order_runnable_refs(
    admission: &ConversationAdmissionStore,
    candidate_refs: BTreeSet<String>,
) -> Vec<String> {
    let mut records = candidate_refs
        .into_iter()
        .filter_map(|reference| {
            let record = admission.invocation_record(&reference)?;
            record.is_runnable().then_some((
                record.admission_sequence,
                record.admitted_at_unix_ms,
                reference,
            ))
        })
        .collect::<Vec<_>>();
    // Root and delegated controls share the retained ledger sequence. Prioritize
    // legacy records (sequence zero) before newly admitted work; timestamps and
    // refs provide the deterministic fallback for records without a sequence.
    records.sort();
    records
        .into_iter()
        .map(|(_, _, reference)| reference)
        .collect()
}

fn default_local_acting_actor_ref(graph: &ResolvedCwrGraph) -> CwrResult<String> {
    let candidates = graph
        .actors
        .iter()
        .filter(|actor| {
            !graph.actor_integrations.iter().any(|integration| {
                integration.actor_ref == actor.actor_ref
                    && integration.adapter_ref != crate::adapter::PERSON_ADAPTER_REF
            })
        })
        .collect::<Vec<_>>();
    let [actor] = candidates.as_slice() else {
        return invalid(format!(
            "local host requires an explicit acting actor; found {} actors without an actor integration",
            candidates.len()
        ));
    };
    Ok(actor.actor_ref.clone())
}

fn optional_default_local_acting_actor_ref(graph: &ResolvedCwrGraph) -> Option<String> {
    let candidates = graph
        .actors
        .iter()
        .filter(|actor| {
            !graph.actor_integrations.iter().any(|integration| {
                integration.actor_ref == actor.actor_ref
                    && integration.adapter_ref != crate::adapter::PERSON_ADAPTER_REF
            })
        })
        .collect::<Vec<_>>();
    let [actor] = candidates.as_slice() else {
        return None;
    };
    Some(actor.actor_ref.clone())
}

pub(crate) fn local_actor_identity(actor: &ResolvedCwrActor) -> CwrResult<LocalActorIdentity> {
    Ok(LocalActorIdentity {
        actor_ref: actor.actor_ref.clone(),
        coworker_ref: actor.coworker_ref.clone(),
        handle: format!("@{}", actor.coworker_slug),
        name: actor
            .actor_name
            .clone()
            .or_else(|| actor.coworker_name.clone())
            .unwrap_or_else(|| actor.coworker_slug.clone()),
        category: actor.category,
        provider_model: actor.provider_model.clone(),
    })
}

fn local_catalog(graph: &ResolvedCwrGraph, acting_actor_ref: &str) -> CwrResult<LocalCatalog> {
    let known_actors = graph
        .actors
        .iter()
        .map(|actor| runtime_actor_identity(graph, actor))
        .collect::<CwrResult<Vec<_>>>()?;
    let mut known_handles = BTreeMap::new();
    for actor in &known_actors {
        if actor.handle == "@cwr" && actor.actor_ref != INTRINSIC_CWR_ACTOR_REF {
            return invalid(format!(
                "actor ref {} uses the reserved @cwr handle",
                actor.actor_ref
            ));
        }
        if let Some(existing_actor_ref) =
            known_handles.insert(actor.handle.clone(), actor.actor_ref.clone())
        {
            return invalid(format!(
                "local handle {} resolves both {existing_actor_ref} and {}",
                actor.handle, actor.actor_ref
            ));
        }
    }
    let acting_actor = known_actors
        .iter()
        .find(|actor| actor.actor_ref == acting_actor_ref)
        .cloned()
        .ok_or_else(|| {
            invalid_error(format!(
                "local acting actor {acting_actor_ref} is not in the resolved graph"
            ))
        })?;
    let mut actors = Vec::new();
    for actor in &graph.actors {
        let handle = runtime_actor_identity(graph, actor)?.handle;
        if actor.actor_ref == acting_actor.actor_ref {
            continue;
        }
        let integrations = graph
            .actor_integrations
            .iter()
            .filter(|integration| integration.actor_ref == actor.actor_ref)
            .collect::<Vec<_>>();
        let integration = match actor.default_actor_integration_ref.as_deref() {
            Some(reference) => integrations
                .iter()
                .copied()
                .find(|binding| binding.actor_integration_ref == reference)
                .ok_or_else(|| invalid_error("declared default integration is unavailable"))?,
            // Partial loading can remove a declared default. Keep the coworker
            // discoverable without assigning another binding to implicit work.
            None => continue,
        };
        // An unavailable default stays selected. Admission reports its failure;
        // choosing a different available provider would change the requested target.
        actors.push(LocalActor {
            actor_ref: actor.actor_ref.clone(),
            coworker_ref: actor.coworker_ref.clone(),
            actor_integration_ref: integration.actor_integration_ref.clone(),
            handle,
            name: actor
                .actor_name
                .clone()
                .or_else(|| actor.coworker_name.clone())
                .unwrap_or_else(|| actor.coworker_slug.clone()),
            category: actor.category,
            provider_model: actor.provider_model.clone(),
        });
    }
    let actor_integrations = actors
        .iter()
        .map(|actor| {
            (
                actor.actor_ref.as_str(),
                actor.actor_integration_ref.as_str(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let actor_handles = actors
        .iter()
        .map(|actor| (actor.actor_ref.as_str(), actor.handle.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut actions = Vec::new();
    let mut activations = BTreeMap::new();
    for route in &graph.routes {
        if actor_integrations.get(route.actor_ref.as_str()).copied()
            != Some(route.actor_integration_ref.as_str())
            || !route.is_available()
        {
            continue;
        }
        let activation = format!(
            "{}--{}",
            actor_handles[route.actor_ref.as_str()],
            route.action_name
        );
        if let Some(existing_actor_ref) =
            activations.insert(activation.clone(), route.actor_ref.clone())
        {
            return invalid(format!(
                "local action {activation} resolves both {existing_actor_ref} and {}",
                route.actor_ref
            ));
        }
        actions.push(LocalAction {
            activation,
            actor_ref: route.actor_ref.clone(),
            actor_integration_ref: route.actor_integration_ref.clone(),
            action_name: route.action_name.clone(),
            title: route.action_title.clone(),
            description: route.action_description.clone(),
            input_schema: route.input_schema.clone(),
            output_schema: route.output_schema.clone(),
        });
    }
    Ok(LocalCatalog {
        default_conversation_target_ref: graph
            .actors
            .iter()
            .find(|actor| actor.actor_ref == acting_actor.actor_ref)
            .and_then(|actor| actor.default_conversation_target_ref.clone()),
        acting_actor,
        known_actors,
        retained_actors: Vec::new(),
        actors,
        actions,
    })
}

fn local_addressing(
    catalog: &LocalCatalog,
    text: &str,
) -> CwrResult<(ConversationIngressAddressing, Option<String>, String)> {
    local_addressing_with_files(catalog, text, false)
}

fn local_addressing_with_files(
    catalog: &LocalCatalog,
    text: &str,
    has_files: bool,
) -> CwrResult<(ConversationIngressAddressing, Option<String>, String)> {
    let trimmed = text.trim();
    let leading = trimmed.split_ascii_whitespace().next().unwrap_or_default();
    let lookup = if leading.eq_ignore_ascii_case("@cwr") {
        "@cwr".to_owned()
    } else if leading
        .get(.."@cwr--".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("@cwr--"))
        && leading.len() > "@cwr--".len()
    {
        // Intrinsic CWR commands have historically accepted a case-insensitive
        // handle. Keep the authored text while normalizing only route lookup.
        format!("@cwr--{}", &leading["@cwr--".len()..])
    } else {
        leading.to_owned()
    };
    if let Some(action) = catalog.action(&lookup) {
        let raw_input = trimmed[leading.len()..].trim();
        let input = parse_schema_argument_text(&action.input_schema, raw_input)?;
        return Ok((
            ConversationIngressAddressing::Action {
                actor_ref: action.actor_ref.clone(),
                activation: action.activation.clone(),
                action_name: action.action_name.clone(),
                input,
            },
            Some(action.actor_integration_ref.clone()),
            trimmed.into(),
        ));
    }
    if let Some(actor) = catalog.actor_by_handle(&lookup) {
        let message_text = trimmed[leading.len()..].trim();
        if message_text.is_empty() && !has_files {
            return invalid("an addressed local message must contain text after its handle");
        }
        return Ok((
            ConversationIngressAddressing::Addressed {
                actor_refs: vec![actor.actor_ref.clone()],
            },
            Some(actor.actor_integration_ref.clone()),
            message_text.into(),
        ));
    }
    if let Some(actor) = catalog.known_actor_by_handle(&lookup) {
        let message_text = trimmed[leading.len()..].trim();
        if message_text.is_empty() && !has_files {
            return invalid("an addressed local message must contain text after its handle");
        }
        return Ok((
            ConversationIngressAddressing::Addressed {
                actor_refs: vec![actor.actor_ref.clone()],
            },
            None,
            message_text.into(),
        ));
    }
    if leading.starts_with('@') {
        return invalid(format!("unknown local actor or action {leading}"));
    }
    Ok((
        ConversationIngressAddressing::Unaddressed,
        None,
        text.into(),
    ))
}

fn group_has_stopping_execution(
    state: &LocalRuntimeState,
    target: &ActionInvocationRecord,
) -> bool {
    if target.target_actor_ref == INTRINSIC_CWR_ACTOR_REF {
        return false;
    }
    let Some(group) = execution_group_ref(&state.admission, target) else {
        return false;
    };
    state.admission.invocation_records().any(|record| {
        record.invocation_ref != target.invocation_ref
            && record.state == ActionInvocationState::Stopping
            && execution_group_ref(&state.admission, record) == Some(group)
    })
}
