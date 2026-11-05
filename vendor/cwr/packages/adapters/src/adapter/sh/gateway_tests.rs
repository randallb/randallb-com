#[test]
fn shell_gateway_callback_checks_input_and_response_correlation() {
    for (allowed, valid_input, matching_reply) in [
        (true, true, true),
        (false, true, true),
        (true, false, true),
        (true, true, false),
    ] {
        let operation = AdapterHostOperation {
            name: "cwr.send_request".into(),
            description: "Fixture".into(),
            input_schema: json!({"type":"object","required":["resource_ref"],"additionalProperties":false,"properties":{"resource_ref":{"const":"fixture"}}}),
            output_schema: json!({"type":"object","required":["status"],"properties":{"status":{"type":"integer"}}}),
        };
        let mut bridge = ShellCoworkerBridge::bind(&operation).unwrap();
        let socket = bridge.socket().to_path_buf();
        let client = thread::spawn(move || {
            let mut socket = CoworkerStream::connect(socket).unwrap();
            let mut reader = CoworkerReader::new(socket.try_clone().unwrap());
            if allowed && valid_input {
                write_adapter_message(&mut socket, &json!({"type":"describe_request"})).unwrap();
                let description: Value = read_adapter_message(&mut reader).unwrap().unwrap();
                assert_eq!(
                    description["input_schema"]["properties"]["resource_ref"]["const"],
                    "fixture"
                );
            }
            write_adapter_message(&mut socket,&json!({"type":"send_request","input":{"resource_ref":if valid_input {"fixture"}else{"forbidden"}}})).unwrap();
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            line
        });
        let frame = AdapterHostMessage::CallResult {
            invocation_ref: if matching_reply { "parent" } else { "other" }.into(),
            call_id: "shell-gateway-request-1".into(),
            result: Some(json!({"status":200})),
            error: None,
        };
        let mut input = Cursor::new(format!("{}\n", serde_json::to_string(&frame).unwrap()));
        let mut output = Vec::new();
        let result = {
            let mut events = ShellAdapterEvents {
                input: ShellHostInput::Direct(&mut input),
                output: &mut output,
                invocation_ref: "parent",
                source_actor_ref: None,
                next_sequence: 2,
                progress_fd: None,
                gateway_operation: allowed.then_some(operation),
                coworker_actions_available: false,
            };
            let until = Instant::now() + Duration::from_secs(3);
            loop {
                let result = bridge.poll(&mut events, until);
                if bridge.handled || result.is_err() {
                    break result;
                }
                assert!(Instant::now() < until);
                thread::sleep(Duration::from_millis(5));
            }
        };
        drop(bridge);
        let line = client.join().unwrap();
        if !allowed || !valid_input {
            assert!(output.is_empty());
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap()["type"],
                "error"
            );
        } else if matching_reply {
            assert!(result.unwrap());
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap()["result"]["status"],
                200
            );
        } else {
            assert!(result.is_err());
            assert!(line.is_empty());
        }
    }
}

#[test]
fn gateway_only_callback_rejects_coworker_discovery_without_a_host_call() {
    let operation = AdapterHostOperation {
        name: "cwr.send_request".into(),
        description: "Fixture".into(),
        input_schema: json!({}),
        output_schema: json!({}),
    };
    let mut bridge = ShellCoworkerBridge::bind(&operation).unwrap();
    let socket = bridge.socket().to_path_buf();
    let client = thread::spawn(move || {
        let mut socket = CoworkerStream::connect(socket).unwrap();
        write_adapter_message(
            &mut socket,
            &json!({"type":"describe_action", "activation":"@helper--run--task"}),
        )
        .unwrap();
        read_adapter_message::<Value>(&mut CoworkerReader::new(socket))
            .unwrap()
            .unwrap()
    });
    let mut input = Cursor::new(Vec::<u8>::new());
    let mut output = Vec::new();
    let mut events = ShellAdapterEvents {
        input: ShellHostInput::Direct(&mut input),
        output: &mut output,
        invocation_ref: "parent",
        source_actor_ref: None,
        next_sequence: 2,
        progress_fd: None,
        gateway_operation: Some(operation),
        coworker_actions_available: false,
    };
    poll_bridge_until_handled(&mut bridge, &mut events);
    assert_eq!(
        client.join().unwrap(),
        json!({"type":"error", "message":"coworker actions are unavailable"})
    );
    assert!(
        output.is_empty(),
        "must not emit undeclared host operations"
    );
}
