use super::*;

#[test]
fn failed_delivery_is_retried_by_the_runtime() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).expect("inspect runtime").runtime.clone();
    let root = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "delivery-root",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "root",
        )
        .expect("create delivery conversation");
    let reply = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "delivery-reply",
            crate::test_refs::actor_ref("person").as_str(),
            Some(root.conversation_ref()),
            "reply",
        )
        .expect("append delivery message");
    let attachment_ref = "cwp://example.test/runtime/attachments/retry";
    seed_attachment(
        &state,
        attachment_ref,
        &root,
        Some(SLACK_BINDING_KEY),
        &root.record.source_message_ref,
    );
    let (adapter_client, adapter_server) = UnixStream::pair().expect("create adapter wire pair");
    let adapter_timeout = adapter_client
        .try_clone()
        .expect("clone adapter timeout handle");
    lock_state(&state)
        .expect("register test adapter")
        .communication_adapters
        .insert(
            SLACK_INTEGRATION_REF.into(),
            RegisteredCommunicationAdapter {
                _session_ref: "cwp://example.test/sessions/retry".into(),
                destinations: Vec::new(),
                writer: client_writer(adapter_server).unwrap(),
            },
        );

    attachments::request_deliveries(&state, root.conversation_ref())
        .expect("request first delivery");
    let mut lines = BufReader::new(adapter_client).lines();
    let first: RuntimeServiceEvent = serde_json::from_str(
        &lines
            .next()
            .expect("first delivery event")
            .expect("read first delivery event"),
    )
    .expect("decode first delivery event");
    assert!(matches!(first, RuntimeServiceEvent::DeliverMessage { .. }));

    let (command_client, command_server) = UnixStream::pair().expect("create command wire pair");
    execute_command(
        &state,
        &client_writer(command_server).unwrap(),
        RuntimeServiceCommand::DeliveryFailed {
            permanent: false,
            retry_after: None,
            version: RUNTIME_SERVICE_VERSION,
            request_id: "failed-delivery".into(),
            attachment_ref: attachment_ref.into(),
            delivery_ref: reply.record.source_message_ref.clone(),
            error: r#"{"outcome":"retryable","retryAfter":18446744073709551615}"#.into(),
        },
    )
    .expect("schedule retry");
    let response: RuntimeServiceEvent = serde_json::from_str(
        &BufReader::new(command_client)
            .lines()
            .next()
            .expect("delivery failure response")
            .expect("read delivery failure response"),
    )
    .expect("decode delivery failure response");
    assert!(matches!(
        response,
        RuntimeServiceEvent::CommandSucceeded { .. }
    ));
    adapter_timeout
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    let retried: RuntimeServiceEvent = serde_json::from_str(
        &lines
            .next()
            .expect("retried delivery event")
            .expect("read retried delivery event"),
    )
    .expect("decode retried delivery event");
    let RuntimeServiceEvent::DeliverMessage { request } = retried else {
        panic!("runtime must retry the failed delivery")
    };
    assert_eq!(request.delivery_ref, reply.record.source_message_ref);

    let (command_client, command_server) = UnixStream::pair().expect("create command wire pair");
    let retry_after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 2_000;
    let suppressed = serde_json::json!({
        "outcome": "retryable",
        "source": "relay",
        "retryAfter": retry_after,
        "report": false,
    });
    assert!(!should_report_delivery_failure(Some(&suppressed)));
    execute_command(
        &state,
        &client_writer(command_server).unwrap(),
        RuntimeServiceCommand::DeliveryFailed {
            permanent: false,
            retry_after: Some(retry_after),
            version: RUNTIME_SERVICE_VERSION,
            request_id: "retryable-failure".into(),
            attachment_ref: attachment_ref.into(),
            delivery_ref: reply.record.source_message_ref.clone(),
            error: suppressed.to_string(),
        },
    )
    .expect("schedule retryable diagnostic");
    let response: RuntimeServiceEvent = serde_json::from_str(
        &BufReader::new(command_client)
            .lines()
            .next()
            .expect("retryable failure response")
            .expect("read retryable failure response"),
    )
    .expect("decode retryable failure response");
    assert!(matches!(
        response,
        RuntimeServiceEvent::CommandSucceeded { .. }
    ));
    adapter_timeout
        .set_read_timeout(Some(std::time::Duration::from_millis(1_200)))
        .expect("set early retry timeout");
    assert!(lines.next().expect("early retry read").is_err());
    adapter_timeout
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .expect("set due retry timeout");
    let retried: RuntimeServiceEvent = serde_json::from_str(
        &lines
            .next()
            .expect("retryable delivery event")
            .expect("read retryable delivery event"),
    )
    .expect("decode retryable delivery event");
    let RuntimeServiceEvent::DeliverMessage { request } = retried else {
        panic!("runtime must retry the retryable diagnostic")
    };
    assert_eq!(request.delivery_ref, reply.record.source_message_ref);
}

#[test]
fn successful_delivery_survives_adapter_reconnect_and_runtime_restart() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).expect("inspect runtime").runtime.clone();
    let root = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "successful-root",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "root",
        )
        .expect("create delivery conversation");
    let reply = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "successful-reply",
            crate::test_refs::actor_ref("person").as_str(),
            Some(root.conversation_ref()),
            "reply",
        )
        .expect("append delivery message");
    let attachment_ref = "cwp://example.test/runtime/attachments/successful";
    seed_attachment(
        &state,
        attachment_ref,
        &root,
        Some(SLACK_BINDING_KEY),
        &root.record.source_message_ref,
    );
    let (adapter_client, adapter_server) = UnixStream::pair().expect("create adapter wire pair");
    adapter_client
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    lock_state(&state)
        .expect("register test adapter")
        .communication_adapters
        .insert(
            SLACK_INTEGRATION_REF.into(),
            RegisteredCommunicationAdapter {
                _session_ref: "cwp://example.test/sessions/successful".into(),
                destinations: Vec::new(),
                writer: client_writer(adapter_server).unwrap(),
            },
        );
    attachments::request_deliveries(&state, root.conversation_ref()).expect("request delivery");
    let mut adapter_reader = BufReader::new(adapter_client);
    let mut line = String::new();
    adapter_reader.read_line(&mut line).expect("read delivery");
    let delivered: RuntimeServiceEvent = serde_json::from_str(&line).unwrap();
    let RuntimeServiceEvent::DeliverMessage { request } = delivered else {
        panic!("runtime must request the reply delivery")
    };
    assert_eq!(request.delivery_ref, reply.record.source_message_ref);

    let (command_client, command_server) = UnixStream::pair().expect("create command wire pair");
    execute_command(
        &state,
        &client_writer(command_server).unwrap(),
        RuntimeServiceCommand::DeliverySucceeded {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "successful-delivery".into(),
            attachment_ref: attachment_ref.into(),
            delivery_ref: reply.record.source_message_ref.clone(),
        },
    )
    .expect("retain successful delivery");
    let response: RuntimeServiceEvent = serde_json::from_str(
        &BufReader::new(command_client)
            .lines()
            .next()
            .expect("delivery success response")
            .expect("read delivery success response"),
    )
    .expect("decode delivery success response");
    assert!(matches!(
        response,
        RuntimeServiceEvent::CommandSucceeded { .. }
    ));
    {
        let retained_state = lock_state(&state).unwrap();
        let retained = &retained_state.attachments[attachment_ref];
        assert!(retained
            .terminal_delivery_refs
            .contains(&reply.record.source_message_ref));
        assert!(!retained
            .requested_delivery_refs
            .contains(&reply.record.source_message_ref));
    }

    attachments::replay_for_adapter(&state, SLACK_INTEGRATION_REF)
        .expect("replay after adapter reconnect");
    line.clear();
    assert!(
        adapter_reader.read_line(&mut line).is_err(),
        "successful delivery must not replay after reconnect"
    );

    let path = lock_state(&state).unwrap().attachment_state_path.clone();
    let stored = attachments::load(&path).expect("read retained attachments");
    assert!(stored.attachments[attachment_ref]
        .terminal_delivery_refs
        .contains(&reply.record.source_message_ref));
    drop(adapter_reader);
    drop(runtime);
    drop(state);

    let reopened = runtime_state(&temporary);
    lock_state(&reopened).unwrap().attachments = stored.attachments;
    let (restarted_client, restarted_server) =
        UnixStream::pair().expect("create restarted adapter wire pair");
    restarted_client
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    lock_state(&reopened)
        .unwrap()
        .communication_adapters
        .insert(
            SLACK_INTEGRATION_REF.into(),
            RegisteredCommunicationAdapter {
                _session_ref: "cwp://example.test/sessions/restarted".into(),
                destinations: Vec::new(),
                writer: client_writer(restarted_server).unwrap(),
            },
        );
    attachments::replay_for_adapter(&reopened, SLACK_INTEGRATION_REF)
        .expect("replay after runtime restart");
    let mut restarted_reader = BufReader::new(restarted_client);
    line.clear();
    assert!(
        restarted_reader.read_line(&mut line).is_err(),
        "successful delivery must not replay after restart"
    );
}

#[test]
fn permanent_delivery_failure_survives_adapter_reconnect_and_runtime_restart() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).expect("inspect runtime").runtime.clone();
    let root = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "permanent-root",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "root",
        )
        .expect("create delivery conversation");
    let reply = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "permanent-reply",
            crate::test_refs::actor_ref("person").as_str(),
            Some(root.conversation_ref()),
            "reply",
        )
        .expect("append delivery message");
    let attachment_ref = "cwp://example.test/runtime/attachments/permanent";
    seed_attachment(
        &state,
        attachment_ref,
        &root,
        Some(SLACK_BINDING_KEY),
        &root.record.source_message_ref,
    );
    let (adapter_client, adapter_server) = UnixStream::pair().expect("create adapter wire pair");
    adapter_client
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    lock_state(&state)
        .expect("register test adapter")
        .communication_adapters
        .insert(
            SLACK_INTEGRATION_REF.into(),
            RegisteredCommunicationAdapter {
                _session_ref: "cwp://example.test/sessions/permanent".into(),
                destinations: Vec::new(),
                writer: client_writer(adapter_server).unwrap(),
            },
        );
    attachments::request_deliveries(&state, root.conversation_ref())
        .expect("request first delivery");
    let mut adapter_reader = BufReader::new(adapter_client);
    let mut line = String::new();
    adapter_reader
        .read_line(&mut line)
        .expect("read first delivery");
    assert!(matches!(
        serde_json::from_str::<RuntimeServiceEvent>(&line).unwrap(),
        RuntimeServiceEvent::DeliverMessage { .. }
    ));

    let (command_client, command_server) = UnixStream::pair().expect("create command wire pair");
    execute_command(
        &state,
        &client_writer(command_server).unwrap(),
        RuntimeServiceCommand::DeliveryFailed {
            permanent: true,
            retry_after: None,
            version: RUNTIME_SERVICE_VERSION,
            request_id: "permanent-failure".into(),
            attachment_ref: attachment_ref.into(),
            delivery_ref: reply.record.source_message_ref.clone(),
            error: r#"{"outcome":"permanent","source":"surge","retryOutcome":"operator_required"}"#
                .into(),
        },
    )
    .expect("retain permanent failure");
    let response: RuntimeServiceEvent = serde_json::from_str(
        &BufReader::new(command_client)
            .lines()
            .next()
            .expect("permanent failure response")
            .expect("read permanent failure response"),
    )
    .expect("decode permanent failure response");
    assert!(matches!(
        response,
        RuntimeServiceEvent::CommandSucceeded { .. }
    ));
    {
        let retained_state = lock_state(&state).unwrap();
        let retained = &retained_state.attachments[attachment_ref];
        assert!(retained
            .terminal_delivery_refs
            .contains(&reply.record.source_message_ref));
        assert!(!retained
            .requested_delivery_refs
            .contains(&reply.record.source_message_ref));
    }
    line.clear();
    assert!(
        adapter_reader.read_line(&mut line).is_err(),
        "no automatic retry"
    );

    attachments::replay_for_adapter(&state, SLACK_INTEGRATION_REF)
        .expect("replay after adapter reconnect");
    line.clear();
    assert!(
        adapter_reader.read_line(&mut line).is_err(),
        "no reconnect retry"
    );

    let path = lock_state(&state).unwrap().attachment_state_path.clone();
    let stored = attachments::load(&path).expect("read retained attachments");
    assert!(stored.attachments[attachment_ref]
        .terminal_delivery_refs
        .contains(&reply.record.source_message_ref));
    drop(adapter_reader);
    drop(runtime);
    drop(state);

    let reopened = runtime_state(&temporary);
    lock_state(&reopened).unwrap().attachments = stored.attachments;
    let (restarted_client, restarted_server) =
        UnixStream::pair().expect("create restarted adapter wire pair");
    restarted_client
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    lock_state(&reopened)
        .unwrap()
        .communication_adapters
        .insert(
            SLACK_INTEGRATION_REF.into(),
            RegisteredCommunicationAdapter {
                _session_ref: "cwp://example.test/sessions/restarted".into(),
                destinations: Vec::new(),
                writer: client_writer(restarted_server).unwrap(),
            },
        );
    attachments::replay_for_adapter(&reopened, SLACK_INTEGRATION_REF)
        .expect("replay after runtime restart");
    let mut restarted_reader = BufReader::new(restarted_client);
    line.clear();
    assert!(
        restarted_reader.read_line(&mut line).is_err(),
        "no restart retry"
    );
}
