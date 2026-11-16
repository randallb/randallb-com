#[test]
fn configured_gateway_process_round_trip_and_failures() {
    let _lock = SH_ADAPTER_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let adapter = temp.path().join("gateway-adapter");
    let started_marker = temp.path().join("gateway-started");
    fs::write(&adapter, r#"#!/usr/bin/env python3
import sys, json, os, time
inv = json.loads(sys.stdin.readline())['invocation']
if 'context_stream' in inv.get('thread', {}):
    while json.loads(sys.stdin.readline())['type'] != 'context_end': pass
request = inv['input']
value = request['request'].get('value', 'status')
if value == 'wait':
    time.sleep(30)
response = dict(type='result', invocation_ref=inv['invocation_ref'], sequence=1, status=200, summary='Done')
if value == 'fail':
    response.update(status=500, error={'code':'test_failure','message':'private failure detail'})
else:
    response['result'] = {'echo':value,'ref':request['ref'],'caller':request['requester_actor_ref'],'pid':os.getpid()}
    if value == 'bad-output': response['result'] = 'wrong'
print(json.dumps(response), flush=True)
"#.replace("time.sleep(30)", &format!("open({:?}, 'w').close(); time.sleep(30)", started_marker))).unwrap();
    fs::set_permissions(&adapter, fs::Permissions::from_mode(0o700)).unwrap();
    let _env = GatewayTestEnvironment(std::env::var_os("CWR_SH_ADAPTER_EXECUTABLE"));
    std::env::set_var("CWR_SH_ADAPTER_EXECUTABLE", &adapter);
    let manifest = gateway_test_workspace(temp.path());
    let gateway_dir = manifest.parent().unwrap().join("coworkers/gateway");
    fs::copy(
        gateway_dir.join("GATEWAY.toml"),
        gateway_dir.join("RELATIVE.toml"),
    )
    .unwrap();
    let mut manifest_source = fs::read_to_string(&manifest).unwrap();
    manifest_source.push_str(
        "\n[[imports]]\npath = './coworkers/gateway/RELATIVE.toml'\nas = 'relative-echo'\n",
    );
    fs::write(&manifest, manifest_source).unwrap();
    let person = load_default_local_acting_actor_ref(&manifest).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        &manifest,
        temp.path().join("runtime.sqlite"),
        LocalHostIdentity::new(person, "cwp://com.example/test"),
    )
    .unwrap();
    // A gateway route must execute even when its handler has no ordinary
    // coworker actor, action route, or binding in the workspace graph.
    {
        let graph = runtime.graph().unwrap();
        let gateway_actor_ref = &graph.gateways[0].route.actor_ref;
        assert!(graph.actors.iter().all(|actor| &actor.actor_ref != gateway_actor_ref));
        assert!(graph.actor_integrations.iter().all(|binding| &binding.actor_ref != gateway_actor_ref));
        assert!(graph.routes.iter().all(|route| &route.actor_ref != gateway_actor_ref));
    }
    let previous = gateway_status_parent(&runtime, None);
    let delivery_ref = {
        let mut state = runtime.lock().unwrap();
        state
            .admission
            .complete(&previous.invocation_ref, json!({"text":"ready"}))
            .unwrap()
            .message_ref
    };
    let caller = gateway_status_parent(&runtime, previous.conversation_ref.as_deref());
    let invoke = |id: &str, input: Value| {
        let AdapterHostCallOutcome::Returned(result) = runtime
            .handle_host_call(
                AdapterCall {
                    invocation_ref: caller.invocation_ref.clone(),
                    call_id: id.into(),
                    operation: "cwr.send_request".into(),
                    input,
                },
                &caller,
                &Rc::new(RefCell::new(VecDeque::new())),
                &AdapterModelOutputMailbox::discard(),
                &mut |_| {},
            )
            .unwrap();
        assert!(
            jsonschema::validator_for(&send_request_operation().output_schema)
                .unwrap()
                .is_valid(&result),
            "{result}"
        );
        result
    };
    let gateway_ref = runtime
        .graph()
        .unwrap()
        .gateways
        .iter()
        .find(|gateway| gateway.route.coworker_source.ends_with("GATEWAY.toml"))
        .unwrap()
        .resource_ref
        .clone();
    let relative_ref = runtime
        .graph()
        .unwrap()
        .gateways
        .iter()
        .find(|gateway| gateway.route.coworker_source.ends_with("RELATIVE.toml"))
        .unwrap()
        .resource_ref
        .clone();
    let input =
        json!({"type":GATEWAY_REQUEST_TYPE,"resource_ref":gateway_ref,"request":{"value":"hello"}});
    let caller_program = temp.path().join("caller-adapter");
    fs::write(&caller_program, r#"#!/usr/bin/env python3
import json, sys
inv = json.loads(sys.stdin.readline())['invocation']
if 'context_stream' in inv.get('thread', {}):
    while json.loads(sys.stdin.readline())['type'] != 'context_end': pass
request = inv['input']
call_id = request.pop('_call_id', 'success')
print(json.dumps(dict(type='call',invocation_ref=inv['invocation_ref'],call_id=call_id,operation='cwr.send_request',input=request)),flush=True)
while True:
    response=json.loads(sys.stdin.readline())
    if response['type']=='call_result': break
print(json.dumps(dict(type='result',invocation_ref=inv['invocation_ref'],sequence=1,status=200,summary='Caller completed',result=response['result'])),flush=True)
"#).unwrap();
    fs::set_permissions(&caller_program, fs::Permissions::from_mode(0o700)).unwrap();
    let mut invocation = AdapterInvocation::new(
        &caller.invocation_ref,
        "test--caller",
        input.clone(),
        json!({"type":"object"}),
    );
    runtime
        .install_lifecycle_operations(&mut invocation, &caller)
        .unwrap();
    assert!(invocation
        .host_operations
        .iter()
        .any(|op| op.name == "cwr.send_request"));
    let outcome = crate::adapter::run_process_adapter_with_controlled_host_calls(
        &crate::adapter::ResolvedProcessAdapter::new(&caller_program),
        invocation,
        &ProcessAdapterStopControl::new(),
        |_| {},
        |call| {
            runtime.handle_host_call(
                call,
                &caller,
                &Rc::new(RefCell::new(VecDeque::new())),
                &AdapterModelOutputMailbox::discard(),
                &mut |_| {},
            )
        },
    )
    .unwrap();
    let ProcessAdapterOutcome::Completed(response) = outcome else {
        panic!("caller stopped");
    };
    let result = response.result.unwrap();
    assert_eq!(result["status"], 200, "{result}");
    assert_eq!(result["result"]["echo"], "hello");
    assert_eq!(result["result"]["caller"], caller.target_actor_ref);
    assert_eq!(result["result"]["ref"], result["request_ref"]);
    assert_ne!(result["result"]["pid"], std::process::id());
    let retained = runtime.lock().unwrap();
    let conversation = retained
        .admission
        .conversation(caller.conversation_ref.as_deref().unwrap())
        .unwrap();
    let gateway_actor_ref = &retained.graph.gateways[0].route.actor_ref;
    assert!(!conversation
        .participant_actor_refs
        .contains(gateway_actor_ref));
    assert!(conversation
        .messages
        .iter()
        .all(|message| !message.addressed_actor_refs.contains(gateway_actor_ref)));
    assert!(conversation.messages.iter().all(|message| &message.author_actor_ref != gateway_actor_ref));
    drop(retained);
    let mut relative = input.clone();
    relative["resource_ref"] = json!(relative_ref);
    let relative_result = invoke("relative", relative);
    assert_eq!(relative_result["status"], 200, "{relative_result}");
    assert!(
        relative_result["result"]["echo"] == "hello",
        "{relative_result}"
    );
    assert_gateway_request_allowlist(&runtime, &caller, &gateway_ref, &relative_ref, &input, &invoke);
    assert_gateway_caller_rejects_malformed_request(&runtime, &caller, &caller_program);
    assert_eq!(invoke("success", input.clone()), result);
    assert_gateway_request_failures(&input, &invoke);
    let mut status_request = input.clone();
    let gateway_alias = runtime.graph().unwrap().gateways[0]
        .request_refs
        .iter()
        .find(|reference| *reference != &gateway_ref)
        .unwrap()
        .clone();
    status_request["resource_ref"] = json!(gateway_alias);
    status_request["request"] = json!({"operation":"delivery_status","delivery_ref":delivery_ref});
    assert_eq!(invoke("status", status_request)["status"], 200);
    assert_gateway_delivery_status_authority(&runtime, &delivery_ref);
    assert_gateway_child_stops_on_reload(&runtime, &manifest, &caller, &started_marker, &relative_ref, &input, &invoke);
    // Stop the parent after its gateway child is admitted; normal descendant
    // cancellation must terminate the separate process and settle the request.
    assert_gateway_child_stops_with_parent(&runtime, &caller, &started_marker, input, invoke);
}

fn assert_gateway_caller_rejects_malformed_request(
    runtime: &LocalRuntime,
    caller: &ActionInvocationRecord,
    caller_program: &Path,
) {
    let mut invocation = AdapterInvocation::new(
        &caller.invocation_ref,
        "test--caller",
        json!({"type":"unsupported", "_call_id":"malformed"}),
        json!({"type":"object"}),
    );
    runtime
        .install_lifecycle_operations(&mut invocation, caller)
        .unwrap();
    let outcome = crate::adapter::run_process_adapter_with_controlled_host_calls(
        &crate::adapter::ResolvedProcessAdapter::new(caller_program),
        invocation,
        &ProcessAdapterStopControl::new(),
        |_| {},
        |call| {
            runtime.handle_host_call(
                call,
                caller,
                &Rc::new(RefCell::new(VecDeque::new())),
                &AdapterModelOutputMailbox::discard(),
                &mut |_| {},
            )
        },
    )
    .unwrap();
    let ProcessAdapterOutcome::Completed(response) = outcome else {
        panic!("caller stopped");
    };
    let result = response.result.unwrap();
    assert_eq!(result["status"], 400, "{result}");
    assert_eq!(result["error"]["code"], "invalid_request", "{result}");
}

fn assert_gateway_child_stops_with_parent(
    runtime: &LocalRuntime,
    caller: &ActionInvocationRecord,
    started_marker: &Path,
    mut input: Value,
    invoke: impl Fn(&str, Value) -> Value,
) {
    let stop_runtime = runtime.clone();
    let parent = caller.invocation_ref.clone();
    let started_marker = started_marker.to_path_buf();
    let stopper = thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if started_marker.exists() {
                stop_runtime.stop_invocation(&parent, None).unwrap();
                break;
            }
            assert!(Instant::now() < until, "gateway child did not start");
            thread::sleep(Duration::from_millis(10));
        }
    });
    input["request"]["value"] = json!("wait");
    assert_eq!(invoke("stop", input)["status"], 499);
    stopper.join().unwrap();
    assert!(runtime.lock().unwrap().active.is_empty());
}


fn assert_gateway_child_stops_on_reload(
    runtime: &LocalRuntime,
    manifest: &Path,
    caller: &ActionInvocationRecord,
    started_marker: &Path,
    gateway_ref: &str,
    input: &Value,
    invoke: &impl Fn(&str, Value) -> Value,
) {
    let reload_runtime = runtime.clone();
    let manifest = manifest.to_owned();
    let marker = started_marker.to_owned();
    let reloader = thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            assert!(Instant::now() < until, "gateway did not start before reload");
            thread::sleep(Duration::from_millis(10));
        }
        let original = fs::read_to_string(&manifest).unwrap();
        let changed = original.replace("\n[[imports]]\npath = './coworkers/gateway/RELATIVE.toml'\nas = 'relative-echo'\n", "");
        assert_ne!(changed, original);
        fs::write(&manifest, changed).unwrap();
        reload_runtime.reload_graph().unwrap();
    });
    let mut request = input.clone();
    request["resource_ref"] = json!(gateway_ref);
    request["request"]["value"] = json!("wait");
    assert_eq!(invoke("reload-stop", request)["status"], 499);
    reloader.join().unwrap();
    fs::remove_file(started_marker).unwrap();
    assert!(!runtime.lock().unwrap().admission.invocation_record(&caller.invocation_ref).unwrap().state.is_terminal());
}
