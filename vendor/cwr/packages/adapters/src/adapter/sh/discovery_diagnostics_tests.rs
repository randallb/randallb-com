    #[test]
    fn coworker_discovery_diagnostics_distinguish_nondefault_and_ambiguous_matches() {
        for (defaults, reason, default_count) in [
            (vec![false], "action_unavailable", 0),
            (vec![true, true], "action_ambiguous", 2),
        ] {
            let mut bridge = ShellCoworkerBridge::bind(&invoke_operation()).unwrap();
            let socket = bridge.socket().to_path_buf();
            let client = thread::spawn(move || {
                let mut stream = CoworkerStream::connect(socket).unwrap();
                write_adapter_message(&mut stream, &json!({
                    "type":"describe_action", "activation":"@helper--run--task"
                })).unwrap();
                read_adapter_message::<Value>(&mut CoworkerReader::new(stream)).unwrap().unwrap()
            });
            let mut discovery = discovery_response();
            let action = discovery["result"]["actions"][0].clone();
            discovery["result"]["actions"] = Value::Array(defaults.iter().map(|default| {
                let mut action = action.clone();
                action["is_default_binding"] = json!(default);
                action["secret"] = json!("do-not-copy-catalog-data");
                action
            }).collect());
            let mut input = Cursor::new(format!("{discovery}\n"));
            let mut output = Vec::new();
            let mut events = ShellAdapterEvents {
            gateway_operation: None,
            coworker_actions_available: true,
                input: ShellHostInput::Direct(&mut input), output: &mut output,
                invocation_ref: "~@/invocations/parent", source_actor_ref: None,
                next_sequence: 2, progress_fd: None,
            };
            poll_bridge_until_handled(&mut bridge, &mut events);
            let response = client.join().unwrap();
            assert_eq!(response["diagnostic"], json!({
                "stage":"discovery", "reason":reason, "catalog_actions":defaults.len(),
                "matching_actions":defaults.len(), "default_matching_actions":default_count
            }));
            assert!(!response.to_string().contains("do-not-copy-catalog-data"));
            let calls: Vec<AdapterEvent> = String::from_utf8(output).unwrap().lines()
                .map(|line| serde_json::from_str(line).unwrap()).collect();
            assert_eq!(calls.len(), 1, "discovery failure must not invoke an action");
        }
    }


    #[test]
    fn discovery_host_error_does_not_expose_untrusted_message_or_code() {
        let response = json!({
            "type":"call_result", "invocation_ref":"~@/invocations/parent",
            "call_id":"shell-coworker-list-1",
            "error":{"code":"PRIVATE_HOST_CODE", "message":"PRIVATE_HOST_CREDENTIAL"}
        });
        let mut input = Cursor::new(format!("{response}\n"));
        let mut output = Vec::new();
        let mut events = ShellAdapterEvents {
            gateway_operation: None,
            coworker_actions_available: true,
            input: ShellHostInput::Direct(&mut input), output: &mut output,
            invocation_ref: "~@/invocations/parent", source_actor_ref: None,
            next_sequence: 2, progress_fd: None,
        };
        let error = events.discover_actions(Instant::now() + Duration::from_secs(2))
            .unwrap_err().to_string();
        assert!(error.contains("host_call_failed"));
        assert!(!error.contains("PRIVATE_HOST"));
    }
