#[test]
fn sms_origin_mail_gateway_preserves_desktop_owner_and_rejects_unowned_or_unrelated_callers() {
    let _lock = SH_ADAPTER_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let adapter = temp.path().join("gateway-bridge-adapter");
    write_gateway_bridge_adapter(&adapter);
    let _env = GatewayTestEnvironment(std::env::var_os("CWR_SH_ADAPTER_EXECUTABLE"));
    std::env::set_var("CWR_SH_ADAPTER_EXECUTABLE", &adapter);
    let manifest = gateway_test_workspace(temp.path());
    let person = load_default_local_acting_actor_ref(&manifest).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        &manifest,
        temp.path().join("runtime.sqlite"),
        LocalHostIdentity::new(person.clone(), "cwp://com.example/test"),
    )
    .unwrap();
    let host = gateway_mail_host();
    let (sender, receiver) = std::sync::mpsc::channel();
    let session = runtime
        .register_upstream_provider(
            "desktop",
            vec![host.clone()],
            Arc::new(move |event| {
                sender
                    .send(event)
                    .map_err(|error| invalid_error(error.to_string()))
            }),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    runtime
        .enable_upstream_coworker("desktop", &session, &host.actor_ref, &person, true)
        .unwrap();
    let caller = gateway_sms_parent(&runtime, &person, "sms-owned", Some("desktop"));
    let gateway_ref = runtime.graph().unwrap().gateways[0].resource_ref.clone();
    {
        let mut state = runtime.lock().unwrap();
        let target = upstream_graph(&state, Some("desktop"))
            .unwrap()
            .routes
            .into_iter()
            .find(|route| route.actor_ref == host.actor_ref)
            .unwrap();
        let denied =
            serde_json::to_string(&route_action_target(&state.graph.runtime_ref, &target)).unwrap();
        let script = fs::read_to_string(&adapter).unwrap().replace(
            "DENIED_TARGET",
            &denied.replace(":true", ":True").replace(":false", ":False"),
        );
        fs::write(&adapter, script).unwrap();
        let binding = state
            .graph
            .actor_integrations
            .iter_mut()
            .find(|binding| {
                Some(binding.actor_integration_ref.as_str())
                    == caller.actor_integration_ref.as_deref()
            })
            .unwrap();
        binding.actor_integration_params["cwr_action_actor_refs"] = json!([gateway_ref]);
    }

    for (event, owner) in [("sms-unowned", None), ("sms-other", Some("other-window"))] {
        let denied_caller = gateway_sms_parent(&runtime, &person, event, owner);
        let AdapterHostCallOutcome::Returned(blocked) = runtime.handle_host_call(
            AdapterCall { invocation_ref: denied_caller.invocation_ref.clone(),
                call_id: event.into(), operation: "cwr.send_request".into(),
                input: json!({"type":GATEWAY_REQUEST_TYPE,"resource_ref":gateway_ref,"request":{"value":"blocked"}}),
            }, &denied_caller, &Rc::new(RefCell::new(VecDeque::new())),
            &AdapterModelOutputMailbox::discard(), &mut |_| {},
        ).unwrap();
        assert_eq!(blocked["result"]["echo"], "blocked", "{blocked}");
    }
    let host_runtime = runtime.clone();
    let worker = thread::spawn(move || {
        for _ in 0..2 {
            let crate::v2026_09::upstream::UpstreamEvent::WorkRequested { work } =
                receiver.recv_timeout(Duration::from_secs(5)).unwrap()
            else {
                panic!("expected host mail work")
            };
            assert_eq!(work.action_name, "read--imap");
            host_runtime
                .complete_upstream_work(
                    "desktop",
                    &work.invocation_ref,
                    AdapterTerminal {
                        invocation_ref: work.invocation_ref.clone(),
                        sequence: 1,
                        status: 200,
                        summary: "Mail read completed".into(),
                        result: Some(json!({"status":200})),
                        error: None,
                        continuation: None,
                    },
                )
                .unwrap();
        }
    });
    let AdapterHostCallOutcome::Returned(result) = runtime
        .handle_host_call(
            AdapterCall {
                invocation_ref: caller.invocation_ref.clone(),
                call_id: "gmail".into(),
                operation: "cwr.send_request".into(),
                input: json!({
                    "type":GATEWAY_REQUEST_TYPE, "resource_ref":gateway_ref,
                    "request":{"value":"gmail"}
                }),
            },
            &caller,
            &Rc::new(RefCell::new(VecDeque::new())),
            &AdapterModelOutputMailbox::discard(),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(result["status"], 200, "{result}");
    assert_eq!(result["result"]["echo"], "200");
    // A second carrier event in the retained conversation must keep the same
    // private Desktop routes after the first manager turn has completed.
    runtime
        .lock()
        .unwrap()
        .admission
        .publish_root_message_completed(
            &caller.invocation_ref,
            ActionResponseStatus::COMPLETED,
            json!({"text":"Mail request finished"}),
            "Done".into(),
            current_unix_ms(),
        )
        .unwrap();
    let continued = gateway_sms_parent_in_conversation(
        &runtime,
        &person,
        "sms-continuation",
        Some("desktop"),
        caller.conversation_ref.as_deref(),
    );
    let AdapterHostCallOutcome::Returned(next) = runtime.handle_host_call(
        AdapterCall { invocation_ref: continued.invocation_ref.clone(), call_id: "continuation-mail".into(),
            operation: "cwr.send_request".into(),
            input: json!({"type":GATEWAY_REQUEST_TYPE,"resource_ref":gateway_ref,"request":{"value":"mail"}}),
        }, &continued, &Rc::new(RefCell::new(VecDeque::new())),
        &AdapterModelOutputMailbox::discard(), &mut |_| {},
    ).unwrap();
    assert_eq!(next["status"], 200, "{next}");
    assert_eq!(next["result"]["echo"], "200");
    assert_eq!(continued.conversation_ref, caller.conversation_ref);
    worker.join().unwrap();
    {
        let mut state = runtime.lock().unwrap();
        let child = state
            .admission
            .invocation_records()
            .find(|record| record.target_actor_ref == host.actor_ref)
            .unwrap()
            .clone();
        state.graph.gateways[0].route.actor_integration_params["cwr_action_actor_refs"] = json!([]);
        assert!(require_current_request_authority(&state, &child)
            .unwrap_err()
            .to_string()
            .contains("no longer authorized"));
    }
    let AdapterHostCallOutcome::Returned(blocked) = runtime.handle_host_call(
        AdapterCall {
            invocation_ref: continued.invocation_ref.clone(), call_id: "blocked".into(),
            operation: "cwr.send_request".into(),
            input: json!({"type":GATEWAY_REQUEST_TYPE,"resource_ref":gateway_ref,"request":{"value":"blocked"}}),
        }, &continued, &Rc::new(RefCell::new(VecDeque::new())),
        &AdapterModelOutputMailbox::discard(), &mut |_| {},
    ).unwrap();
    assert_eq!(blocked["result"]["echo"], "blocked", "{blocked}");

    let state = runtime.lock().unwrap();
    let conversation = state
        .admission
        .conversation(caller.conversation_ref.as_deref().unwrap())
        .unwrap();
    assert!(!conversation
        .participant_actor_refs
        .contains(&state.graph.gateways[0].route.actor_ref));
    assert!(conversation.messages.iter().all(|message| !message
        .addressed_actor_refs
        .contains(&state.graph.gateways[0].route.actor_ref)));
    let gateway_progress = conversation
        .messages
        .iter()
        .filter(|message| message.text == "Gateway request in progress")
        .collect::<Vec<_>>();
    assert_eq!(gateway_progress.len(), 2);
    assert!(gateway_progress
        .iter()
        .all(|message| message.author_actor_ref == caller.target_actor_ref));
    assert!(gateway_progress
        .iter()
        .all(|message| { message.author_actor_ref != state.graph.gateways[0].route.actor_ref }));
}

fn assert_gateway_delivery_status_authority(runtime: &LocalRuntime, delivery_ref: &str) {
    let (status_ref, status_input) = {
        let mut state = runtime.lock().unwrap();
        state.graph.gateways[0].connection_ref = Some("sms-status-test".into());
        let child = state
            .admission
            .invocation_records()
            .find(|record| {
                matches!(&record.origin,
            InvocationOrigin::HostCall { call_id, .. } if call_id == "status")
            })
            .unwrap();
        (child.invocation_ref.clone(), child.input.clone())
    };
    let sms_schema: Value = serde_json::from_str(
        &fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../coworkers/workloop_sms/schemas/request.input.json"
        ))
        .unwrap(),
    )
    .unwrap();
    assert!(jsonschema::validator_for(&sms_schema)
        .unwrap()
        .is_valid(&status_input));
    assert!(runtime
        .gateway_invocation_matches(
            "sms-status-test",
            "",
            "",
            &status_ref,
            crate::v2026_09::service::GatewayInvocationKind::Request,
            &status_input
        )
        .unwrap());
    let other_actor = status_input["requester_actor_ref"].as_str().unwrap();
    let other_request = runtime.admit_text(None, "@helper help").unwrap();
    let other_caller = {
        let mut state = runtime.lock().unwrap();
        match state
            .admission
            .claim_invocation(other_request.invocation_ref().unwrap(), current_unix_ms())
            .unwrap()
        {
            ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
            _ => panic!("other caller was not claimed"),
        }
    };
    assert_eq!(other_caller.target_actor_ref, other_actor);
    let other_status_request = json!({
        "type": GATEWAY_REQUEST_TYPE,
        "resource_ref": runtime.graph().unwrap().gateways[0].resource_ref,
        "request": {"operation": "delivery_status", "delivery_ref": delivery_ref}
    });
    let AdapterHostCallOutcome::Returned(other_result) = runtime
        .handle_host_call(
            AdapterCall {
                invocation_ref: other_caller.invocation_ref.clone(),
                call_id: "other-status".into(),
                operation: "cwr.send_request".into(),
                input: other_status_request,
            },
            &other_caller,
            &Rc::new(RefCell::new(VecDeque::new())),
            &AdapterModelOutputMailbox::discard(),
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(other_result["status"], 200, "{other_result}");
    let (other_status_ref, other_status_input) = {
        let state = runtime.lock().unwrap();
        let child = state
            .admission
            .invocation_records()
            .find(|record| {
                matches!(&record.origin,
                    InvocationOrigin::HostCall { call_id, .. } if call_id == "other-status")
            })
            .unwrap();
        assert_eq!(
            child.parent_invocation_ref.as_deref(),
            Some(other_caller.invocation_ref.as_str())
        );
        assert_eq!(child.requester_actor_ref, other_actor);
        (child.invocation_ref.clone(), child.input.clone())
    };
    assert_eq!(other_status_input["request"]["delivery_ref"], delivery_ref);
    assert!(!runtime
        .gateway_invocation_matches(
            "sms-status-test",
            "",
            "",
            &other_status_ref,
            crate::v2026_09::service::GatewayInvocationKind::Request,
            &other_status_input
        )
        .unwrap());
    let mut forged = status_input.clone();
    forged["requester_actor_ref"] = json!("cwp://com.example/other-caller");
    assert!(!runtime
        .gateway_invocation_matches(
            "sms-status-test",
            "",
            "",
            &status_ref,
            crate::v2026_09::service::GatewayInvocationKind::Request,
            &forged
        )
        .unwrap());
    let mut unrelated_delivery = status_input.clone();
    unrelated_delivery["request"]["delivery_ref"] = json!("cwp://com.example/messages/other");
    assert!(!runtime
        .gateway_invocation_matches(
            "sms-status-test",
            "",
            "",
            &status_ref,
            crate::v2026_09::service::GatewayInvocationKind::Request,
            &unrelated_delivery
        )
        .unwrap());
}

fn assert_gateway_request_failures(input: &Value, invoke: &impl Fn(&str, Value) -> Value) {
    for value in ["fail", "bad-output"] {
        let mut request = input.clone();
        request["request"]["value"] = json!(value);
        let result = invoke(value, request);
        assert_eq!(result["status"], 502, "{result}");
        assert!(!result.to_string().contains("private failure detail"));
    }
    let mut request = input.clone();
    request["resource_ref"] = json!("cwp://com.example/missing");
    assert_eq!(invoke("missing", request)["status"], 404);
    let mut request = input.clone();
    request["request"]["value"] = json!(42);
    assert_eq!(invoke("invalid", request)["status"], 400);
    let mut request = input.clone();
    request["requester_actor_ref"] = json!("cwp://com.example/spoof");
    assert_eq!(invoke("spoof", request)["status"], 400);
}

fn gateway_status_parent(
    runtime: &LocalRuntime,
    conversation: Option<&str>,
) -> ActionInvocationRecord {
    let request = runtime.admit_text(conversation, "@helper help").unwrap();
    let mut state = runtime.lock().unwrap();
    let ConversationInvocationClaimOutcome::Claimed(claimed) = state
        .admission
        .claim_invocation(request.invocation_ref().unwrap(), current_unix_ms())
        .unwrap()
    else {
        panic!("parent was not claimed")
    };
    let adapter = &claimed.adapter_invocation;
    let context = crate::adapter::working_context(&adapter.thread).unwrap();
    state.admission.append_execution_activity(&adapter.invocation_ref,
        crate::v2026_09::conversation::ConversationActivityEvent::ProviderEvent {
            provider: "codex".into(), method: "cwr.working_state".into(),
            data: json!({"version":1,"idle":true,"provider":"codex","thread_id":"test-thread",
                "working_ref":context["working_conversation"]["ref"],
                "invocation_ref":adapter.invocation_ref,"context_snapshot":context["context_snapshot"]}),
        }).unwrap();
    claimed.claim.invocation
}


fn write_gateway_bridge_adapter(adapter: &Path) {
    fs::write(adapter, r#"#!/usr/bin/env python3
import json, os, sys
inv = json.loads(sys.stdin.readline())['invocation']
if 'context_stream' in inv.get('thread', {}):
    while json.loads(sys.stdin.readline())['type'] != 'context_end': pass
def call(call_id, operation, input):
    print(json.dumps(dict(type='call', invocation_ref=inv['invocation_ref'], call_id=call_id, operation=operation, input=input)), flush=True)
    while True:
        response = json.loads(sys.stdin.readline())
        if response['type'] == 'call_result':
            if response.get('error'): raise RuntimeError(response['error'])
            return response['result']
actions = call('discover', 'cwr.list_actions', {})['actions']
if inv['input']['request'].get('value') == 'blocked':
    assert not actions, actions
    try:
        call('invoke-denied', 'cwr.invoke_action', {'target':DENIED_TARGET, 'input':{'operation':'folders'}})
    except RuntimeError as error:
        pass # A forged exact target must fail as well as remain undiscoverable.
    else:
        raise RuntimeError('disallowed action was invoked')
    request = inv['input']
    print(json.dumps(dict(type='result', invocation_ref=inv['invocation_ref'], sequence=1, status=200, summary='Blocked', result={'echo':'blocked','pid':os.getpid(),'ref':request['ref'],'caller':request['requester_actor_ref']})), flush=True)
    sys.exit(0)
target = next(action['target'] for action in actions if action['activation'] == '@workloop_ops--read--imap')
response = call('invoke', 'cwr.invoke_action', {'target':target, 'input':{'operation':'folders'}})
request = inv['input']
print(json.dumps(dict(type='progress', published=True, invocation_ref=inv['invocation_ref'], sequence=1, status=102, summary='Gateway request in progress')), flush=True)
print(json.dumps(dict(type='result', invocation_ref=inv['invocation_ref'], sequence=2, status=200, summary='Done', result={'echo':str(response['status']), 'pid':os.getpid(), 'ref':request['ref'], 'caller':request['requester_actor_ref']})), flush=True)
"#).unwrap();
    fs::set_permissions(adapter, fs::Permissions::from_mode(0o700)).unwrap();
}

fn gateway_mail_host() -> crate::v2026_09::upstream::UpstreamCoworker {
crate::v2026_09::upstream::UpstreamCoworker {
        actor_ref: "cwp://com.example/desktop/actors/workloop_ops".into(),
        coworker_ref: "cwp://com.example/desktop/coworkers/workloop_ops".into(),
        handle: "@workloop_ops".into(),
        name: "Workloop Ops".into(),
        actions: vec![crate::v2026_09::upstream::UpstreamAction {
            action_name: "read--imap".into(),
            title: "Read connected mail".into(),
            description: "Read connected mail".into(),
            input_schema: json!({"type":"object","required":["operation"],"properties":{"operation":{"const":"folders"}},"additionalProperties":false}),
            output_schema: json!({"type":"object"}),
        }],
    }
}
