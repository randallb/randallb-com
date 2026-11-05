const LIST_CONTEXTS_HOST_OPERATION: &str = "cwr.list_conversation_contexts";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListConversationContextsInput {}

fn list_contexts_operation() -> AdapterHostOperation {
    AdapterHostOperation {
        name: LIST_CONTEXTS_HOST_OPERATION.into(),
        description: "Discover enabled conversation destinations that include you. Use owner_context_ref with create_conversation. This does not create conversations or add participants. Missing context policy returns no destinations.".into(),
        input_schema: json!({"x-cwr-model-name":"cwp.cwr--list--conversation_contexts","x-cwr-decoder-version":"cwp/1","type":"object","additionalProperties":false,"properties":{}}),
        output_schema: json!({"type":"object","required":["contexts"],"properties":{"contexts":{"type":"array","items":{"type":"object","required":["owner_context_ref","is_default","actor_refs"],"properties":{"owner_context_ref":{"type":"string"},"is_default":{"type":"boolean"},"actor_refs":{"type":"array","items":{"type":"string"}}}}}}}),
    }
}

impl LocalRuntime {
    fn handle_list_contexts(
        &self,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
    ) -> CwrResult<AdapterHostCallOutcome> {
        let _: ListConversationContextsInput =
            parse_input("list-conversation-contexts", &call.input)?;
        if call.invocation_ref != caller.invocation_ref {
            return invalid("adapter host call belongs to another invocation");
        }
        let state = self.lock()?;
        validate_message_call(&state, call, None)?;
        let contexts: Vec<_> = state
            .admission
            .context_policy
            .as_ref()
            .filter(|policy| !policy.suspended)
            .into_iter()
            .flat_map(|policy| {
                policy
                    .contexts
                    .iter()
                    .filter(|(_, actors)| actors.contains(&caller.target_actor_ref))
                    .map(|(reference, actors)| {
                        json!({
                            "owner_context_ref": reference,
                            "is_default": reference == &policy.default_context_ref,
                            "actor_refs": actors,
                        })
                    })
            })
            .collect();
        Ok(AdapterHostCallOutcome::Returned(
            json!({"contexts": contexts}),
        ))
    }
}
