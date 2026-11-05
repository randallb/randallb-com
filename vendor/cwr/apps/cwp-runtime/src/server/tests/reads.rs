use super::*;

#[test]
fn runtime_service_reads_a_bounded_root_conversation_page() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).expect("inspect runtime").runtime.clone();
    for request_id in ["root-one", "root-two"] {
        runtime
            .submit_text_as(
                "cwp://example.test/integrations/desktop",
                request_id,
                crate::test_refs::actor_ref("person").as_str(),
                None,
                request_id,
            )
            .expect("create root conversation");
    }

    let (client, server) = UnixStream::pair().expect("create read wire pair");
    execute_command(
        &state,
        &client_writer(server).unwrap(),
        RuntimeServiceCommand::ReadRootConversations {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "read-roots".into(),
            page: ConversationSummaryPageRequest {
                first: Some(1),
                ..Default::default()
            },
            timeout_ms: 1_000,
        },
    )
    .expect("read root conversation page");

    let event: RuntimeServiceEvent = serde_json::from_str(
        &BufReader::new(client)
            .lines()
            .next()
            .expect("root page response")
            .expect("read root page response"),
    )
    .expect("decode root page response");
    let RuntimeServiceEvent::RootConversationsPage { request_id, page } = event else {
        panic!("runtime must return the requested root page")
    };
    assert_eq!(request_id, "read-roots");
    assert_eq!(page.summaries.len(), 1);
    assert!(page.has_next_page);
    assert_eq!(page.total_count, 2);
}

#[test]
fn runtime_service_read_deadline_includes_state_lock_wait() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let held_state = lock_state(&state).expect("hold runtime state");
    let (client, server) = UnixStream::pair().expect("create read wire pair");
    client
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("bound deadline response wait");
    execute_command(
        &state,
        &client_writer(server).unwrap(),
        RuntimeServiceCommand::ReadRootConversations {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "expired-read".into(),
            page: ConversationSummaryPageRequest {
                first: Some(1),
                ..Default::default()
            },
            timeout_ms: 10,
        },
    )
    .expect("start root conversation read");

    let event: RuntimeServiceEvent = serde_json::from_str(
        &BufReader::new(client)
            .lines()
            .next()
            .expect("deadline response")
            .expect("read deadline response"),
    )
    .expect("decode deadline response");
    assert!(matches!(
        event,
        RuntimeServiceEvent::CommandFailed { request_id, error }
            if request_id == "expired-read" && error.contains("deadline")
    ));
    drop(held_state);
}

#[test]
fn runtime_service_read_cancellation_interrupts_state_lock_wait() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let held_state = lock_state(&state).expect("hold runtime state");
    let (client, server) = UnixStream::pair().expect("create read wire pair");
    client
        .set_read_timeout(Some(Duration::from_millis(250)))
        .expect("bound cancellation response wait");
    let writer = client_writer(server).unwrap();
    execute_command(
        &state,
        &writer,
        RuntimeServiceCommand::ReadRootConversations {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "cancelled-read".into(),
            page: ConversationSummaryPageRequest {
                first: Some(1),
                ..Default::default()
            },
            timeout_ms: 1_000,
        },
    )
    .expect("start root conversation read");
    execute_command(
        &state,
        &writer,
        RuntimeServiceCommand::CancelRead {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "cancel-read-command".into(),
            target_request_id: "cancelled-read".into(),
        },
    )
    .expect("cancel root conversation read");

    let events = BufReader::new(client)
        .lines()
        .take(2)
        .map(|line| serde_json::from_str::<RuntimeServiceEvent>(&line.unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeServiceEvent::CommandFailed { request_id, error }
            if request_id == "cancelled-read" && error.contains("cancelled")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeServiceEvent::CommandSucceeded { request_id, .. }
            if request_id == "cancel-read-command"
    )));
    drop(held_state);
}
