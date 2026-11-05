// Exercise the production order: claim continuity, install tools, retain,
// settle, then admit another request in the same public conversation.
fn claim_gateway_turn(
    runtime: &LocalRuntime,
    conversation: Option<&str>,
) -> (ActionInvocationRecord, AdapterInvocation) {
    let request = runtime
        .admit_text(conversation, "@helper continue")
        .unwrap();
    let claimed = {
        let mut state = runtime.lock().unwrap();
        // These tests mutate the activated graph directly rather than reloading
        // authored resources. Keep the admission graph in sync as reload does.
        state.admission.response_graph = Some(state.graph.clone());
        let ConversationInvocationClaimOutcome::Claimed(claimed) = state
            .admission
            .claim_invocation(request.invocation_ref().unwrap(), current_unix_ms())
            .unwrap()
        else {
            panic!("claim")
        };
        *claimed
    };
    let record = claimed.claim.invocation;
    let mut invocation = claimed.adapter_invocation;
    runtime
        .install_lifecycle_operations(&mut invocation, &record)
        .unwrap();
    runtime
        .lock()
        .unwrap()
        .admission
        .retain_execution_invocation(&record.invocation_ref, &invocation)
        .unwrap();
    (record, invocation)
}

fn settle_gateway_turn(
    runtime: &LocalRuntime,
    record: &ActionInvocationRecord,
    invocation: &AdapterInvocation,
) {
    let context = crate::adapter::working_context(&invocation.thread).unwrap();
    let mut state = runtime.lock().unwrap();
    state.admission.append_execution_activity(&record.invocation_ref, ConversationActivityEvent::ProviderEvent {
        provider:"codex".into(), method:"cwr.working_state".into(),
        data:json!({"version":1,"idle":true,"provider":"codex","thread_id":"gateway-thread",
            "provider_store":"managed",
            "working_ref":context["working_conversation"]["ref"],
            "invocation_ref":record.invocation_ref,"context_snapshot":context["context_snapshot"]}),
    }).unwrap();
    state
        .admission
        .complete(&record.invocation_ref, json!({"text":"Completed outcome"}))
        .unwrap();
}

fn set_helper_gateways(runtime: &LocalRuntime, allowed: Value) {
    let mut state = runtime.lock().unwrap();
    let helper = state
        .graph
        .actors
        .iter()
        .find(|actor| actor.coworker_slug == "helper")
        .unwrap()
        .actor_ref
        .clone();
    state
        .graph
        .actor_integrations
        .iter_mut()
        .find(|binding| binding.actor_ref == helper)
        .unwrap()
        .actor_integration_params["cwr_action_actor_refs"] = allowed;
}

fn gateway_tool(invocation: &AdapterInvocation) -> &AdapterHostOperation {
    invocation
        .host_operations
        .iter()
        .find(|op| op.name == "cwr.send_request")
        .unwrap()
}

#[test]
fn gateway_continuity_refreshes_approved_access_and_enforces_revocation() {
    let _lock = SH_ADAPTER_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let adapter = temp.path().join("echo-gateway");
    fs::write(&adapter, r#"#!/usr/bin/env python3
import sys, json, os
inv = json.loads(sys.stdin.readline())['invocation']
if 'context_stream' in inv.get('thread', {}):
    while json.loads(sys.stdin.readline())['type'] != 'context_end': pass
request = inv['input']
print(json.dumps(dict(type='result', invocation_ref=inv['invocation_ref'], sequence=1, status=200, summary='Done', result=dict(echo=request['request']['value'], pid=os.getpid(), ref=request['ref'], caller=request['requester_actor_ref']))), flush=True)
"#).unwrap();
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o700)).unwrap();
    let _env = GatewayTestEnvironment(std::env::var_os("CWR_SH_ADAPTER_EXECUTABLE"));
    std::env::set_var("CWR_SH_ADAPTER_EXECUTABLE", &adapter);
    let manifest = gateway_test_workspace(temp.path());
    let person = load_default_local_acting_actor_ref(&manifest).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        &manifest,
        temp.path().join("runtime.sqlite"),
        LocalHostIdentity::new(person, "cwp://com.example/test"),
    )
    .unwrap();
    let gateway_ref = runtime.graph().unwrap().gateways[0].resource_ref.clone();
    set_helper_gateways(&runtime, json!([]));
    let (first, first_adapter) = claim_gateway_turn(&runtime, None);
    assert_eq!(
        gateway_tool(&first_adapter).input_schema["properties"]["resource_ref"],
        false
    );
    settle_gateway_turn(&runtime, &first, &first_adapter);
    set_helper_gateways(&runtime, json!([gateway_ref]));
    let (connected, connected_adapter) =
        claim_gateway_turn(&runtime, first.conversation_ref.as_deref());
    let working = &connected_adapter.thread["working_conversation"];
    assert!(working["history_gap"]
        .as_str()
        .unwrap()
        .contains("gateway contracts changed"));
    assert!(working.get("state").is_none());
    assert_ne!(
        working["ref"],
        first_adapter.thread["working_conversation"]["ref"]
    );
    assert_eq!(connected.conversation_ref, first.conversation_ref);
    assert!(serde_json::to_string(&connected_adapter.thread)
        .unwrap()
        .contains("Completed outcome"));
    let input = json!({"type":GATEWAY_REQUEST_TYPE,"resource_ref":gateway_ref,"request":{"value":"new access"}});
    assert!(
        jsonschema::validator_for(&gateway_tool(&connected_adapter).input_schema)
            .unwrap()
            .is_valid(&input)
    );
    let call = |caller: &ActionInvocationRecord, id: &str| {
        let AdapterHostCallOutcome::Returned(result) = runtime
            .handle_host_call(
                AdapterCall {
                    invocation_ref: caller.invocation_ref.clone(),
                    call_id: id.into(),
                    operation: "cwr.send_request".into(),
                    input: input.clone(),
                },
                caller,
                &Rc::new(RefCell::new(VecDeque::new())),
                &AdapterModelOutputMailbox::discard(),
                &mut |_| {},
            )
            .unwrap();
        result
    };
    assert_eq!(
        call(&connected, "new-gateway")["result"]["echo"],
        "new access"
    );
    settle_gateway_turn(&runtime, &connected, &connected_adapter);
    let completed_children = runtime
        .lock()
        .unwrap()
        .admission
        .invocation_records()
        .filter(|record| record.parent_invocation_ref.as_deref() == Some(&connected.invocation_ref))
        .count();
    assert_eq!(completed_children, 1);

    let (unchanged, unchanged_adapter) =
        claim_gateway_turn(&runtime, first.conversation_ref.as_deref());
    assert_eq!(
        unchanged_adapter.thread["working_conversation"]["ref"],
        working["ref"]
    );
    assert_eq!(
        unchanged_adapter.thread["working_conversation"]["state"]["thread_id"],
        "gateway-thread"
    );
    assert_eq!(
        gateway_tool(&unchanged_adapter).input_schema["properties"],
        gateway_tool(&connected_adapter).input_schema["properties"]
    );
    // Revocation while a retained session is active must still reject new calls.
    set_helper_gateways(&runtime, json!([]));
    assert_eq!(call(&unchanged, "revoked-live")["status"], 403);
    settle_gateway_turn(&runtime, &unchanged, &unchanged_adapter);
    let (disconnected, disconnected_adapter) =
        claim_gateway_turn(&runtime, first.conversation_ref.as_deref());
    assert!(disconnected_adapter.thread["working_conversation"]
        .get("state")
        .is_none());
    assert_eq!(
        gateway_tool(&disconnected_adapter).input_schema["properties"]["resource_ref"],
        false
    );
    assert_eq!(call(&disconnected, "revoked-next")["status"], 403);
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_records()
            .filter(
                |record| record.parent_invocation_ref.as_deref() == Some(&connected.invocation_ref)
            )
            .count(),
        completed_children
    );

    settle_gateway_turn(&runtime, &disconnected, &disconnected_adapter);
    // A retry preserves its bound tools and source boundary when authority
    // changes between attempts. New calls still obey current authorization.
    set_helper_gateways(&runtime, json!([gateway_ref]));
    let (retry_record, retry_adapter) =
        claim_gateway_turn(&runtime, first.conversation_ref.as_deref());
    runtime
        .lock()
        .unwrap()
        .admission
        .recover_interrupted_invocations()
        .unwrap();
    set_helper_gateways(&runtime, json!([]));
    let retried = {
        let mut state = runtime.lock().unwrap();
        state.admission.response_graph = Some(state.graph.clone());
        let ConversationInvocationClaimOutcome::Claimed(claimed) = state
            .admission
            .claim_invocation(&retry_record.invocation_ref, current_unix_ms())
            .unwrap()
        else {
            panic!("retry")
        };
        claimed
    };
    assert_eq!(retried.claim.invocation.attempt_count, 2);
    assert_eq!(retried.adapter_invocation.thread, retry_adapter.thread);
    assert_eq!(
        retried.adapter_invocation.host_operations,
        retry_adapter.host_operations
    );
    assert_eq!(
        call(&retried.claim.invocation, "revoked-retry")["status"],
        403
    );
}

#[test]
fn gateway_continuity_refreshes_changed_request_schema() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = gateway_test_workspace(temp.path());
    let person = load_default_local_acting_actor_ref(&manifest).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        &manifest,
        temp.path().join("runtime.sqlite"),
        LocalHostIdentity::new(person, "cwp://com.example/test"),
    )
    .unwrap();
    let (first, first_adapter) = claim_gateway_turn(&runtime, None);
    settle_gateway_turn(&runtime, &first, &first_adapter);
    runtime.lock().unwrap().graph.gateways[0].route.input_schema["properties"]["request"]
        ["required"] = json!(["value"]);
    let (_, next) = claim_gateway_turn(&runtime, first.conversation_ref.as_deref());
    assert!(next.thread["working_conversation"].get("state").is_none());
    assert_ne!(
        gateway_tool(&next).input_schema["properties"]["request"],
        gateway_tool(&first_adapter).input_schema["properties"]["request"]
    );
    assert!(
        gateway_tool(&next).input_schema["properties"]["request"]["description"]
            .as_str()
            .unwrap()
            .contains("required")
    );
}
