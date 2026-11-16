impl LocalRuntime {
    /// The caller must be the actor bound by the trusted host session. No
    /// feature flag check here: disabling creation cannot strand pending work.
    pub fn reply_person_request(&self, actor_ref: &str, conversation_ref: &str, invocation_ref: &str, text: String) -> CwrResult<LocalAdmission> {
        self.reply_person_request_in_upstream_scope(actor_ref, conversation_ref, invocation_ref, text, None)
    }

    pub fn reply_person_request_in_upstream_scope(&self, actor_ref: &str, conversation_ref: &str, invocation_ref: &str, text: String, owner: Option<&str>) -> CwrResult<LocalAdmission> {
        // Select new software work outside the runtime lock, just as ordinary
        // host admission does. Readiness can invoke an external auth check.
        let (graph, target) = {
            let state = self.lock()?;
            let graph = upstream_graph(&state, owner)?;
            let request = state.admission.invocation_record(invocation_ref)
                .filter(|r| r.target_actor_ref == actor_ref && r.conversation_ref.as_deref() == Some(conversation_ref))
                .ok_or_else(|| invalid_error("response request does not belong to this person and conversation"))?;
            let target = if request.state == ActionInvocationState::AwaitingPerson && request.input.get("provider_question").is_none() { Some(request.requester_actor_ref.clone()) } else { None };
            (graph, target)
        };
        let preferred = target.as_deref().map(|target| graph.preferred_message_binding_with(target, |binding| Ok(binding.process_adapter_route()?.readiness()))).transpose()?.flatten();
        let route_snapshot = match (&target, &preferred) {
            (Some(target), Some(binding)) => Some(graph.message_binding_route_snapshot(target, binding)?),
            _ => None,
        };
        let mut state = self.lock()?;
        let mut current_graph = upstream_graph(&state, owner)?;
        if let Some(target) = &target {
            let previous = graph.actors.iter().find(|a| &a.actor_ref == target);
            let current = current_graph.actors.iter().find(|a| &a.actor_ref == target);
            if previous.map(|a| (&a.message_binding_preference, &a.default_actor_integration_ref)) != current.map(|a| (&a.message_binding_preference, &a.default_actor_integration_ref)) {
                return invalid("message route changed; retry the reply");
            }
            if let Some(binding) = &preferred {
                if Some(current_graph.message_binding_route_snapshot(target, binding)?) != route_snapshot {
                    return invalid("message route changed; retry the reply");
                }
                if let Some(actor) = current_graph.actors.iter_mut().find(|a| &a.actor_ref == target) {
                    actor.default_actor_integration_ref = Some(binding.clone());
                }
            }
        }
        if let Some(target) = &target {
            let binding = current_graph.actors.iter().find(|a| &a.actor_ref == target)
                .and_then(|a| a.default_actor_integration_ref.as_ref())
                .and_then(|reference| current_graph.actor_integrations.iter().find(|b| &b.actor_ref == target && &b.actor_integration_ref == reference));
            if !binding.is_some_and(|b| b.is_available() && b.message_mode == crate::v2026_09::graph::ActorIntegrationMessageMode::Conversational) {
                return invalid("requesting coworker has no available message route; retry after restoring its binding");
            }
        }
        if state.shutting_down { return invalid("local runtime is shutting down"); }
        if text.trim().is_empty() { return invalid("reply must not be blank"); }
        if !state.graph.actors.iter().any(|actor| actor.actor_ref == actor_ref) { return invalid("selected person is unavailable"); }
        let record = state.admission.admissions().find(|record| record.invocation_ref.as_deref() == Some(invocation_ref))
            .filter(|record| record.adapter_ref.as_deref() == Some(crate::adapter::PERSON_ADAPTER_REF)
                && record.target_actor_ref.as_deref() == Some(actor_ref)
                && record.conversation_ref == conversation_ref)
            .cloned().ok_or_else(|| invalid_error("response request does not belong to this person and conversation"))?;
        let invocation = state.admission.invocation_record(invocation_ref).ok_or_else(|| invalid_error("response request is unavailable"))?;
        let reply_result = if let Some(metadata) = invocation.input.get("provider_question") {
            let caller = metadata["caller"].as_str().ok_or("question has no provider invocation")?;
            require_no_stop_fence(&state, caller)?;
            if state.admission.invocation_record(caller).is_none_or(|r| r.state.is_terminal()) {
                return invalid("question is no longer waiting for an answer");
            }
            let questions = crate::adapter::user_input::validate_questions(&metadata["questions"])?;
            let answers = crate::adapter::user_input::validate_answers(&questions, &text)?;
            let readable = questions.iter().map(|q| format!("{}: {}", q.header,
                if q.is_secret { "[private answer]".into() } else { answers.answers[&q.id].answers.join(", ") }))
                .collect::<Vec<_>>().join("\n");
            json!({"text":readable,"answers_response":answers})
        } else { json!({"text":text}) };
        if let Some(response) = invocation.terminal_response() {
            if !response.status.is_completed() { return invalid("response request is no longer pending"); }
            if response.result.as_ref() != Some(&reply_result) {
                return invalid("response request already has a different reply");
            }
        } else {
            if invocation.state != ActionInvocationState::AwaitingPerson { return invalid("response request is not awaiting a person"); }
            let previous_graph = state.admission.response_graph.replace(current_graph);
            let result = state.admission.publish_root_message_completed(invocation_ref, ActionResponseStatus::COMPLETED,
                reply_result, "Replied".into(), current_unix_ms());
            state.admission.response_graph = previous_graph;
            result?;
        }
        let record = state.admission.admission(&record.integration_ref, &record.provider_event_ref)
            .cloned().ok_or_else(|| invalid_error("response admission disappeared"))?;
        let conversation = state.admission.full_conversation(conversation_ref)?.ok_or_else(|| invalid_error("conversation disappeared"))?;
        if let (Some(owner), Some(reply_ref)) = (owner, record.reply_message_ref.as_ref()) {
            let continuations = state.admission.admissions().filter(|r| &r.source_message_ref == reply_ref)
                .filter_map(|r| r.invocation_ref.clone()).collect::<Vec<_>>();
            for reference in continuations {
                state.host_local_invocation_owners.entry(reference).or_insert_with(|| owner.to_owned());
            }
        }
        Ok(LocalAdmission { record, conversation })
    }
}
