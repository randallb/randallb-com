const CREATE_CONVERSATION_HOST_OPERATION: &str = "cwr.create_conversation";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateConversationInput {
    owner_context_ref: String,
    text: String,
    #[serde(default)]
    file_refs: Vec<String>,
}

fn create_conversation_operation() -> AdapterHostOperation {
    AdapterHostOperation {
        name: CREATE_CONVERSATION_HOST_OPERATION.into(),
        description: "Create a new conversation in a pack or Workspace context. Discover eligible owner_context_ref values with cwp.cwr--list--conversation_contexts. Pass only the task context needed there; history is not copied. Returns admission references without waiting for the reply.".into(),
        input_schema: json!({"x-cwr-model-name":"cwp.cwr--create--conversation","x-cwr-decoder-version":"cwp/1","type":"object","additionalProperties":false,"required":["owner_context_ref","text"],"properties":{
            "owner_context_ref":{"type":"string","minLength":1},
            "text":{"type":"string"},
            "file_refs":{"type":"array","items":{"type":"string","minLength":1}}
        }}),
        output_schema: json!({"type":"object","required":["conversation_ref","message_ref"],"properties":{
            "conversation_ref":{"type":"string"},"message_ref":{"type":"string"},"invocation_ref":{"type":["string","null"]}
        }}),
    }
}

impl LocalRuntime {
    fn handle_create_conversation(
        &self,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
    ) -> CwrResult<AdapterHostCallOutcome> {
        let input: CreateConversationInput = parse_input("create-conversation", &call.input)?;
        if input.owner_context_ref.trim().is_empty() {
            return invalid("owner_context_ref must not be blank");
        }
        let owner = {
            let state = self.lock()?;
            validate_message_call(&state, call, None)?;
            state
                .host_local_invocation_owners
                .get(&caller.invocation_ref)
                .cloned()
        };
        let admission = self.submit_message_from_call(
            INTRINSIC_CWR_INTEGRATION_REF,
            format!("{}#{}", call.invocation_ref, call.call_id),
            &caller.target_actor_ref,
            None,
            input.text,
            input.file_refs,
            owner.as_deref(),
            Some(&input.owner_context_ref),
            Some((call, caller)),
        )?;
        Ok(AdapterHostCallOutcome::Returned(json!({
            "conversation_ref": admission.conversation_ref(),
            "message_ref": admission.record.source_message_ref,
            "invocation_ref": admission.invocation_ref(),
        })))
    }
}

fn validate_create_conversation_call(
    state: &LocalRuntimeState,
    graph: &ResolvedCwrGraph,
    call: &AdapterCall,
    caller: &ActionInvocationRecord,
    ingress: &ConversationIngress,
) -> CwrResult<()> {
    validate_message_call(state, call, None)?;
    let allowed = action_actor_allowlist(
        graph,
        &caller.target_actor_ref,
        caller.actor_integration_ref.as_deref(),
    )?;
    let actor_refs = match &ingress.addressing {
        ConversationIngressAddressing::Addressed { actor_refs } => actor_refs.as_slice(),
        ConversationIngressAddressing::Action { actor_ref, .. } => std::slice::from_ref(actor_ref),
        ConversationIngressAddressing::Unaddressed => &[],
        ConversationIngressAddressing::UnresolvedLeadingHandle { .. } => {
            return invalid("coworker request has an unresolved target");
        }
    };
    if allowed.as_ref().is_some_and(|allowed| {
        actor_refs
            .iter()
            .any(|actor| actor != &caller.target_actor_ref && !allowed.contains(actor))
    }) {
        return invalid("coworker request is not authorized");
    }
    Ok(())
}
