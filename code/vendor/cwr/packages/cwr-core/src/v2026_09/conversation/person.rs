/// Route only newly completed conversational replies. Admission and completion
/// commit together, so restart never has to scan or replay historical messages.
fn stage_person_response_routes(state: &mut ConversationRuntimeState, graph: &ResolvedCwrGraph) -> CwrResult<()> {
    use crate::v2026_09::invocation::ActionInvocationState;
    let completed = state.invocation_ledger.changes().filter_map(|(_, old, new)| {
        let record = new?;
        (record.state == ActionInvocationState::Completed
            && old.is_none_or(|old| old.state != ActionInvocationState::Completed)
            && record.action_name == RESPOND_TO_MESSAGE_ACTION_NAME).then(|| record.clone())
    }).collect::<Vec<_>>();
    for record in completed {
        if record.input.get("provider_question").is_some() { continue; }
        let Some(conversation_ref) = record.conversation_ref.as_deref() else { continue };
        let Some(conversation) = state.conversations.iter().find(|c| c.conversation_ref == conversation_ref).cloned() else { continue };
        let execution = state.invocation_execution_conversation_refs.get(&record.invocation_ref)
            .and_then(|reference| state.conversations.iter().find(|c| &c.conversation_ref == reference));
        let source = execution.and_then(|c| c.activity.iter().find_map(|activity| match &activity.event {
            ConversationActivityEvent::TurnStarted { source_message_ref, invocation, .. } => Some((source_message_ref.clone(), invocation.as_ref().is_some_and(|i| i.pointer("/retained_route/adapter_ref").and_then(Value::as_str) == Some(crate::adapter::PERSON_ADAPTER_REF)))),
            _ => None,
        }));
        let Some((source_ref, from_person)) = source else { continue };
        let Some(message) = conversation.messages.iter().find(|m| m.reply_to_message_ref.as_deref() == Some(&source_ref) && m.author_actor_ref == record.target_actor_ref && m.progress.is_none()).cloned() else { continue };
        let Some(target) = message.addressed_actor_refs.first() else { continue };
        let Some(actor) = graph.actors.iter().find(|actor| &actor.actor_ref == target) else { continue };
        let Some(binding) = graph.actor_integrations.iter().find(|b| &b.actor_ref == target && Some(&b.actor_integration_ref) == actor.default_actor_integration_ref.as_ref()) else { continue };
        if !from_person && binding.adapter_ref != crate::adapter::PERSON_ADAPTER_REF { continue }
        if !binding.is_available() || binding.message_mode != crate::v2026_09::graph::ActorIntegrationMessageMode::Conversational { continue }
        stage_reply_request(state, conversation, &message, binding)?;
    }
    Ok(())
}

fn stage_reply_request(state: &mut ConversationRuntimeState, conversation: RetainedConversation, message: &ConversationMessage, binding: &ResolvedCwrActorIntegration) -> CwrResult<()> {
    let new_ref = |kind| format!("{}/{kind}/{}", state.runtime_root_ref, Uuid::new_v4().simple());
    let invocation_ref = new_ref("invocations");
    let execution_ref = new_ref("conversations");
    let invocation = binding.project_adapter_invocation(invocation_ref.clone(), RESPOND_TO_MESSAGE_ACTION_NAME,
        json!({"source_message_ref":message.message_ref}), respond_to_message_output_schema(), conversation_thread(&conversation));
    let mut invocation = invocation;
    invocation.source_actor_ref = Some(message.author_actor_ref.clone());
    let execution = RetainedConversation {
        owner_context_ref: conversation.owner_context_ref.clone(),
        conversation_ref: execution_ref.clone(), title: None, archived: false,
        sidebar_eligible: None,
        parent_conversation_ref: Some(conversation.conversation_ref.clone()),
        participant_actor_refs: vec![binding.actor_ref.clone()], messages: vec![],
        timeline_connection_ref: None,
        activity: vec![ConversationActivity {
            activity_ref: new_ref("activities"), sequence: 1, occurred_at_unix_ms: current_unix_ms(),
            event: ConversationActivityEvent::TurnStarted {
                invocation_ref: invocation_ref.clone(), source_message_ref: message.message_ref.clone(),
                invocation: Some(context::invocation_snapshot(&invocation)?),
            },
        }],
    };
    let record = ConversationAdmissionRecord {
        integration_ref: format!("{}/response-routing", state.runtime_root_ref),
        provider_event_ref: message.message_ref.clone(), ingress_fingerprint: None,
        source_input_fingerprint: None,
        conversation_ref: conversation.conversation_ref.clone(), source_message_ref: message.message_ref.clone(),
        execution_conversation_ref: Some(execution_ref), target_actor_ref: Some(binding.actor_ref.clone()),
        actor_integration_ref: Some(binding.actor_integration_ref.clone()), adapter_ref: Some(binding.adapter_ref.clone()),
        invocation_ref: Some(invocation_ref), state: ConversationAdmissionState::Admitted,
        reply_recipient_visibility: ConversationRecipientVisibility::Explicit, reply_message_ref: None,
    };
    if state.admissions.iter().any(|r| r.integration_ref == record.integration_ref && r.provider_event_ref == record.provider_event_ref) { return Ok(()) }
    let prepared = PreparedConversationAdmission { record: record.clone(), conversation, execution_conversation: Some(execution.clone()), invocation: Some(invocation) };
    invocation::stage_root_invocation(state, &prepared)?;
    state.conversations.push(execution);
    state.admissions.push(record);
    Ok(())
}
