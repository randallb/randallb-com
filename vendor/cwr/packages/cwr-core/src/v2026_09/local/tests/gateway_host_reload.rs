fn admit_native_reload_child(
    runtime: &LocalRuntime,
    person: &str,
    event: &str,
) -> (ActionInvocationRecord, String) {
    let caller = gateway_sms_parent(runtime, person, event, Some("desktop"));
    let route = runtime.graph().unwrap().gateways[0].route.clone();
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: event.into(),
        operation: "cwr.send_request".into(),
        input: json!({}),
    };
    let envelope = serde_json::to_value(GatewayRequest {
        r#type: GATEWAY_REQUEST_TYPE.into(),
        r#ref: format!("cwp://example.test/{event}"),
        requester_actor_ref: caller.target_actor_ref.clone(),
        parent_invocation_ref: caller.invocation_ref.clone(),
        resource_ref: route.actor_ref.clone(),
        request: json!({"value":"synthetic"}),
    })
    .unwrap();
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    let AdapterHostCallOutcome::Returned(started) = runtime
        .start_gateway_action(&call, &caller, &queue, &route, envelope)
        .unwrap();
    let child: StartActionResult = serde_json::from_value(started).unwrap();
    (caller, child.invocation_ref)
}

fn native_reload_child(
    runtime: &LocalRuntime,
    person: &str,
    event: &str,
) -> (
    ActionInvocationRecord,
    String,
    std::thread::JoinHandle<CwrResult<()>>,
) {
    let (caller, child) = admit_native_reload_child(runtime, person, event);
    let executing = runtime.clone();
    let reference = child.clone();
    let worker = thread::spawn(move || {
        executing.execute_requested_ref(
            &reference,
            &AdapterModelOutputMailbox::discard(),
            &mut |_| {},
        )
    });
    (caller, child, worker)
}

fn native_success(reference: &str) -> AdapterTerminal {
    AdapterTerminal {
        invocation_ref: reference.into(),
        sequence: 1,
        status: 200,
        summary: "Synthetic read".into(),
        result: Some(json!({"echo":"private","pid":0,"ref":"synthetic","caller":"synthetic"})),
        error: None,
        continuation: None,
    }
}

fn change_native_gateway(temp: &tempfile::TempDir, runtime: &LocalRuntime, authority: bool) {
    let directory = temp.path().join("workspace/coworkers/gateway");
    if authority {
        let path = directory.join("GATEWAY.toml");
        let old = fs::read_to_string(&path).unwrap();
        fs::write(path, format!("authority = [\"process.execute\"]\n{old}")).unwrap();
    } else {
        let path = directory.join("output.json");
        let mut schema: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        schema["properties"]["new_optional_field"] = json!({"type":"string"});
        fs::write(path, schema.to_string()).unwrap();
    }
    runtime.reload_graph().unwrap();
}

#[test]
fn native_gateway_reload_discards_late_terminals_and_replaces_without_rebinding() {
    for (authority, status) in [(false, 200), (true, 403), (false, 422)] {
        let (temp, runtime, person) = native_gateway_fixture();
        let (sender, receiver) = std::sync::mpsc::channel();
        let writer: UpstreamWriter = Arc::new(move |event| {
            sender
                .send(event)
                .map_err(|error| invalid_error(error.to_string()))
        });
        let stop = || std::sync::mpsc::sync_channel(1).0;
        let original = runtime
            .register_gateway_handlers(
                "desktop",
                vec![native_gateway_declaration(&runtime)],
                writer.clone(),
                stop(),
            )
            .unwrap();
        let (_caller, child, worker) = native_reload_child(&runtime, &person, "reload-inflight");
        let UpstreamEvent::WorkRequested { .. } =
            receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected work")
        };
        runtime.protect_upstream_result("desktop", &child).unwrap();
        let definition = temp
            .path()
            .join("workspace/coworkers/gateway")
            .join(if authority {
                "GATEWAY.toml"
            } else {
                "output.json"
            });
        let original_definition = fs::read_to_string(&definition).unwrap();
        change_native_gateway(&temp, &runtime, authority);
        {
            let state = runtime.lock().unwrap();
            assert!(require_native_gateway_result_current(
                &state,
                state.admission.invocation_record(&child).unwrap(),
            )
            .is_err());
        }
        let mut replacement = runtime
            .register_gateway_handlers(
                "desktop",
                vec![native_gateway_declaration(&runtime)],
                writer.clone(),
                stop(),
            )
            .unwrap();
        assert_ne!(original, replacement);
        {
            let state = runtime.lock().unwrap();
            assert!(require_native_gateway_result_current(
                &state,
                state.admission.invocation_record(&child).unwrap(),
            )
            .is_err());
        }
        // Restoring the old graph must not revive its withdrawn executor.
        fs::write(&definition, original_definition).unwrap();
        runtime.reload_graph().unwrap();
        let restored = runtime
            .register_gateway_handlers(
                "desktop",
                vec![native_gateway_declaration(&runtime)],
                writer.clone(),
                stop(),
            )
            .unwrap();
        assert_ne!(restored, original);
        assert_ne!(restored, replacement);
        replacement = restored;
        {
            let state = runtime.lock().unwrap();
            assert!(require_native_gateway_result_current(
                &state,
                state.admission.invocation_record(&child).unwrap(),
            )
            .is_err());
        }
        let UpstreamEvent::StopRequested { invocation_ref } =
            receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected abort");
        };
        assert_eq!(invocation_ref, child);
        let mut late = native_success(&child);
        late.status = status;
        late.summary = "PRIVATE_LATE_OUTPUT summary".into();
        late.result.as_mut().unwrap()["echo"] = json!("PRIVATE_LATE_OUTPUT");
        if status != 200 {
            late.result = None;
            late.error = Some(crate::adapter::AdapterError {
                code: "PRIVATE_LATE_OUTPUT_error".into(),
                message: Some("PRIVATE_LATE_OUTPUT details".into()),
                details: Some(json!({"secret":"PRIVATE_LATE_OUTPUT"})),
            });
        }
        if status == 422 {
            let mut invalid = late.clone();
            invalid.result = Some(json!({"secret":"PRIVATE_LATE_OUTPUT invalid"}));
            assert!(runtime
                .complete_upstream_work("desktop", &child, invalid)
                .is_err());
        }
        // Late success/failure settles the old provider without publishing any
        // terminal data or restoring its original authority.
        runtime
            .complete_upstream_work("desktop", &child, late)
            .unwrap();
        worker.join().unwrap().unwrap();
        {
            let state = runtime.lock().unwrap();
            let retained = state.admission.execution_conversation(&child).unwrap();
            assert!(!serde_json::to_string(retained)
                .unwrap()
                .contains("PRIVATE_LATE_OUTPUT"));
            let record = state.admission.invocation_record(&child).unwrap();
            assert_eq!(record.state, ActionInvocationState::Stopped);
            assert!(!serde_json::to_string(record.terminal_response().unwrap())
                .unwrap()
                .contains("PRIVATE_LATE_OUTPUT"));
        }
        let (_next, next_child, next_worker) =
            native_reload_child(&runtime, &person, "reload-next");
        let UpstreamEvent::WorkRequested { work } =
            receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected new work")
        };
        assert_eq!(work.route.provider_session_ref, replacement);
        runtime
            .complete_upstream_work("desktop", &next_child, native_success(&next_child))
            .unwrap();
        next_worker.join().unwrap().unwrap();
    }
}

#[test]
fn native_gateway_reload_revokes_completed_child_before_atomic_parent_publication() {
    for (atomic, status) in [
        (false, ActionResponseStatus::COMPLETED),
        (true, ActionResponseStatus::COMPLETED),
        (true, ActionResponseStatus::FAILED),
        (true, ActionResponseStatus::new(422).unwrap()),
    ] {
        let (temp, runtime, person) = native_gateway_fixture();
        let (sender, receiver) = std::sync::mpsc::channel();
        runtime
            .register_gateway_handlers(
                "desktop",
                vec![native_gateway_declaration(&runtime)],
                Arc::new(move |event| {
                    sender
                        .send(event)
                        .map_err(|error| invalid_error(error.to_string()))
                }),
                std::sync::mpsc::sync_channel(1).0,
            )
            .unwrap();
        let (caller, child, worker) = native_reload_child(&runtime, &person, "reload-published");
        let UpstreamEvent::WorkRequested { .. } =
            receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected work")
        };
        runtime.protect_upstream_result("desktop", &child).unwrap();
        runtime
            .complete_upstream_work("desktop", &child, native_success(&child))
            .unwrap();
        worker.join().unwrap().unwrap();
        assert!(
            runtime.lock().unwrap().protected_upstream_results[&caller.invocation_ref]
                .contains_key(&child)
        );
        if atomic {
            // Complete a real positive host validation, then change the graph
            // before the final publication lock is acquired.
            let validating = runtime.clone();
            let parent = caller.clone();
            let validation =
                thread::spawn(move || validating.check_upstream_result_delivery(&parent));
            let UpstreamEvent::ValidateResult { validation_ref, .. } =
                receiver.recv_timeout(Duration::from_secs(2)).unwrap()
            else {
                panic!("expected result validation")
            };
            runtime
                .validate_upstream_result("desktop", &validation_ref, true)
                .unwrap();
            assert!(validation.join().unwrap().unwrap());
        }
        change_native_gateway(&temp, &runtime, false);
        assert!(runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(&caller.invocation_ref)
            .unwrap()
            .state
            .is_terminal());
        // Every terminal category can contain provider-derived adapter text.
        if atomic {
            runtime
                .publish_invocation_terminal(
                    &caller,
                    InvocationTerminal {
                        status,
                        summary: "Private answer".into(),
                        result: status
                            .is_completed()
                            .then(|| json!({"text":"Private answer"})),
                        error: (!status.is_completed()).then(|| {
                            ActionResponseDetail::new("private_error")
                                .with_message("Private answer")
                        }),
                    },
                    "Private answer".into(),
                    None,
                    &Rc::new(RefCell::new(VecDeque::new())),
                    &mut |_| {},
                )
                .unwrap();
        } else {
            runtime
                .publish_terminal(
                    &caller,
                    AdapterTerminal {
                        invocation_ref: caller.invocation_ref.clone(),
                        sequence: 1,
                        status: 200,
                        summary: "Private answer".into(),
                        result: Some(json!({"text":"Private answer"})),
                        error: None,
                        continuation: None,
                    },
                    &Rc::new(RefCell::new(VecDeque::new())),
                    &mut |_| {},
                )
                .unwrap();
        }
        let state = runtime.lock().unwrap();
        let retained = state
            .admission
            .conversation(caller.conversation_ref.as_deref().unwrap())
            .unwrap();
        assert!(retained
            .messages
            .iter()
            .all(|message| !message.text.contains("Private answer")));
        assert!(!serde_json::to_string(retained)
            .unwrap()
            .contains("Private answer"));
        let response = state
            .admission
            .invocation_record(&caller.invocation_ref)
            .unwrap()
            .terminal_response()
            .unwrap();
        assert!(!serde_json::to_string(response)
            .unwrap()
            .contains("Private answer"));
        assert!(!state
            .protected_upstream_results
            .contains_key(&caller.invocation_ref));
    }
}

#[test]
fn native_gateway_reload_aborts_and_settles_only_after_provider_cleanup() {
    for change in ["schema", "gateway_authority", "caller_authority"] {
        let (temp, runtime, person) = native_gateway_fixture();
        let (sender, receiver) = std::sync::mpsc::channel();
        let original = runtime
            .register_gateway_handlers(
                "desktop",
                vec![native_gateway_declaration(&runtime)],
                Arc::new(move |event| {
                    sender
                        .send(event)
                        .map_err(|error| invalid_error(error.to_string()))
                }),
                std::sync::mpsc::sync_channel(1).0,
            )
            .unwrap();
        let (caller, child, worker) = native_reload_child(&runtime, &person, "abort-cleanup");
        let UpstreamEvent::WorkRequested { work } =
            receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected work");
        };
        assert_eq!(work.route.provider_session_ref, original);
        runtime.protect_upstream_result("desktop", &child).unwrap();
        if change == "caller_authority" {
            let path = temp.path().join("workspace/coworkers/helper/coworker.toml");
            let mut definition = fs::read_to_string(&path).unwrap();
            definition.push_str("\n[integrations.params]\ncwr_action_actor_refs = []\n");
            fs::write(path, definition).unwrap();
            runtime.reload_graph().unwrap();
        } else {
            change_native_gateway(&temp, &runtime, change == "gateway_authority");
        }
        // Receiving abort and finishing cleanup are separate provider events.
        let UpstreamEvent::StopRequested { invocation_ref } =
            receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("expected abort");
        };
        assert_eq!(invocation_ref, child);
        worker.join().unwrap().unwrap();
        {
            let state = runtime.lock().unwrap();
            let record = state.admission.invocation_record(&child).unwrap();
            assert_eq!(record.state, ActionInvocationState::Stopping);
            assert!(record.finished_at_unix_ms.is_none());
            assert!(state.upstream_work.contains_key(&child));
            assert!(!state
                .admission
                .invocation_record(&caller.invocation_ref)
                .unwrap()
                .state
                .is_terminal());
        }
        assert!(runtime.take_settled_stop_invocations().unwrap().is_empty());
        assert!(runtime.acknowledge_upstream_stop("other", &child).is_err());
        // A replacement's registration must not steal the old control route.
        let replacement = runtime
            .register_gateway_handlers(
                "desktop",
                vec![native_gateway_declaration(&runtime)],
                Arc::new(|_| Ok(())),
                std::sync::mpsc::sync_channel(1).0,
            )
            .unwrap();
        if change == "caller_authority" {
            assert_eq!(replacement, original);
        } else {
            assert_ne!(replacement, original);
        }
        runtime
            .acknowledge_upstream_stop("desktop", &child)
            .unwrap();
        {
            let state = runtime.lock().unwrap();
            let record = state.admission.invocation_record(&child).unwrap();
            assert_eq!(record.state, ActionInvocationState::Stopped);
            assert!(record.finished_at_unix_ms.is_some());
            assert!(!state.upstream_work.contains_key(&child));
            assert!(!serde_json::to_string(record)
                .unwrap()
                .contains("PRIVATE_LATE_OUTPUT"));
        }
        assert!(runtime
            .take_settled_stop_invocations()
            .unwrap()
            .contains(&child));
    }
}

#[test]
fn native_gateway_reload_stops_admitted_work_before_dispatch() {
    let (temp, runtime, person) = native_gateway_fixture();
    let (sender, receiver) = std::sync::mpsc::channel();
    runtime
        .register_gateway_handlers(
            "desktop",
            vec![native_gateway_declaration(&runtime)],
            Arc::new(move |event| {
                sender
                    .send(event)
                    .map_err(|error| invalid_error(error.to_string()))
            }),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    let (_caller, child) = admit_native_reload_child(&runtime, &person, "not-dispatched");
    change_native_gateway(&temp, &runtime, false);
    runtime
        .execute_requested_ref(&child, &AdapterModelOutputMailbox::discard(), &mut |_| {})
        .unwrap();
    assert!(receiver.try_recv().is_err());
    let state = runtime.lock().unwrap();
    let record = state.admission.invocation_record(&child).unwrap();
    assert_eq!(record.state, ActionInvocationState::Stopped);
    assert!(record.finished_at_unix_ms.is_some());
    assert!(!state.upstream_work.contains_key(&child));
}

#[test]
fn native_gateway_caller_authority_restore_cannot_revive_consumed_result() {
    let (temp, runtime, person) = native_gateway_fixture();
    let (sender, receiver) = std::sync::mpsc::channel();
    let writer: UpstreamWriter = Arc::new(move |event| {
        sender
            .send(event)
            .map_err(|error| invalid_error(error.to_string()))
    });
    let session = runtime
        .register_gateway_handlers(
            "desktop",
            vec![native_gateway_declaration(&runtime)],
            writer.clone(),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    let (caller, child, worker) = native_reload_child(&runtime, &person, "consumed-before-revoke");
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::WorkRequested { .. }
    ));
    runtime.protect_upstream_result("desktop", &child).unwrap();
    runtime
        .complete_upstream_work("desktop", &child, native_success(&child))
        .unwrap();
    worker.join().unwrap().unwrap();
    let path = temp.path().join("workspace/coworkers/helper/coworker.toml");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!("{original}\n[integrations.params]\ncwr_action_actor_refs = []\n"),
    )
    .unwrap();
    runtime.reload_graph().unwrap();
    fs::write(&path, original).unwrap();
    runtime.reload_graph().unwrap();
    // Caller-only revocation needs no provider replacement, but the consuming
    // turn's stop decision persists after authority is restored.
    assert_eq!(
        runtime
            .register_gateway_handlers(
                "desktop",
                vec![native_gateway_declaration(&runtime)],
                writer,
                std::sync::mpsc::sync_channel(1).0,
            )
            .unwrap(),
        session
    );
    runtime
        .publish_invocation_terminal(
            &caller,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "PRIVATE_RESTORED_RESULT".into(),
                result: Some(json!({"text":"PRIVATE_RESTORED_RESULT"})),
                error: None,
            },
            "PRIVATE_RESTORED_RESULT".into(),
            None,
            &Rc::new(RefCell::new(VecDeque::new())),
            &mut |_| {},
        )
        .unwrap();
    {
        let state = runtime.lock().unwrap();
        let parent = state
            .admission
            .invocation_record(&caller.invocation_ref)
            .unwrap();
        assert_eq!(parent.state, ActionInvocationState::Stopped);
        assert!(!serde_json::to_string(parent.terminal_response().unwrap())
            .unwrap()
            .contains("PRIVATE_RESTORED_RESULT"));
        assert!(!serde_json::to_string(
            state
                .admission
                .conversation(caller.conversation_ref.as_deref().unwrap())
                .unwrap()
        )
        .unwrap()
        .contains("PRIVATE_RESTORED_RESULT"));
    }
    let (_next_caller, next_child, next_worker) =
        native_reload_child(&runtime, &person, "fresh-after-restore");
    let UpstreamEvent::WorkRequested { work } =
        receiver.recv_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("expected fresh work");
    };
    assert_eq!(work.route.provider_session_ref, session);
    runtime
        .complete_upstream_work("desktop", &next_child, native_success(&next_child))
        .unwrap();
    next_worker.join().unwrap().unwrap();
}
