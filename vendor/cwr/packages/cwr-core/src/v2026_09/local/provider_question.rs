impl LocalRuntime {
    fn handle_provider_question(
        &self,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
    ) -> CwrResult<AdapterHostCallOutcome> {
        use crate::adapter::user_input::validate_questions;
        let mut state = self.lock()?;
        require_no_stop_fence(&state, &caller.invocation_ref)?;
        require_active_caller(&state.admission, call)?;
        if state.shutting_down {
            return invalid("runtime is shutting down");
        }
        let request_key = call.input["key"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("missing provider request key")?;
        let event_ref = format!("{}#question#{request_key}", caller.invocation_ref);
        let existing = state
            .admission
            .admissions()
            .find(|r| {
                r.integration_ref == INTRINSIC_CWR_INTEGRATION_REF
                    && r.provider_event_ref == event_ref
            })
            .cloned();
        if let Some(record) = existing {
            let reference = record
                .invocation_ref
                .as_deref()
                .ok_or("question has no invocation")?;
            let question = state
                .admission
                .invocation_record(reference)
                .ok_or("question is unavailable")?;
            if question
                .input
                .pointer("/provider_question/caller")
                .and_then(Value::as_str)
                != Some(&caller.invocation_ref)
            {
                return invalid("question belongs to another invocation");
            }
            if call.input["action"] == "cancel" {
                state.admission.publish_root_stop(
                    reference,
                    InvocationStopRequest::new(current_unix_ms()),
                    "Question is no longer waiting for an answer.".into(),
                )?;
                return Ok(AdapterHostCallOutcome::Returned(
                    json!({"state":"resolved"}),
                ));
            }
            if call.input["action"] == "start"
                && question.input.pointer("/provider_question/questions")
                    != Some(&call.input["questions"])
            {
                return invalid("provider request ID reused with different questions");
            }
            return Ok(AdapterHostCallOutcome::Returned(
                match question.terminal_response() {
                    Some(response) if response.status.is_completed() => {
                        json!({"state":"answered", "response":response.result.as_ref().and_then(|r| r.get("answers_response"))})
                    }
                    Some(_) => json!({"state":"resolved"}),
                    None => json!({"state":"pending"}),
                },
            ));
        }
        if call.input["action"] != "start" {
            return Ok(AdapterHostCallOutcome::Returned(
                json!({"state":"resolved"}),
            ));
        }
        let questions = validate_questions(&call.input["questions"])?;
        let owner = state
            .host_local_invocation_owners
            .get(&caller.invocation_ref)
            .cloned();
        let graph = upstream_graph(&state, owner.as_deref())?;
        // Address the human who originated this execution, never guess from text
        // or choose an arbitrary viewer in a multi-person conversation.
        let mut root = caller;
        while let Some(parent) = &root.parent_invocation_ref {
            root = state
                .admission
                .invocation_record(parent)
                .ok_or("request ancestry is unavailable")?;
        }
        let person = &root.requester_actor_ref;
        let binding = graph
            .actor_integrations
            .iter()
            .find(|b| {
                &b.actor_ref == person
                    && b.adapter_ref == crate::adapter::PERSON_ADAPTER_REF
                    && b.is_available()
            })
            .ok_or("requesting person has no enabled human response binding")?;
        let allowed = action_actor_allowlist(
            &graph,
            &caller.target_actor_ref,
            caller.actor_integration_ref.as_deref(),
        )?;
        if allowed.as_ref().is_some_and(|refs| !refs.contains(person)) {
            return invalid("requesting person is outside coworker authority");
        }
        let conversation_ref = root
            .conversation_ref
            .as_deref()
            .ok_or("request has no shared conversation")?;
        let text = questions
            .iter()
            .map(|q| q.question.clone())
            .collect::<Vec<_>>()
            .join("\n\n");
        let ingress = ConversationIngress {
            owner_context_ref: None,
            file_refs: Vec::new(),
            integration_ref: INTRINSIC_CWR_INTEGRATION_REF.into(),
            provider_event_ref: event_ref,
            author_actor_ref: caller.target_actor_ref.clone(),
            conversation: ConversationIngressConversation::Continue {
                conversation_ref: conversation_ref.into(),
            },
            addressing: ConversationIngressAddressing::Addressed {
                actor_refs: vec![person.clone()],
            },
            text,
        };
        state.admission.admit_provider_question(
            &graph,
            &ingress,
            &binding.actor_integration_ref,
            json!({"caller":caller.invocation_ref,"questions":call.input["questions"]}),
        )?;
        Ok(AdapterHostCallOutcome::Returned(json!({"state":"pending"})))
    }
}
