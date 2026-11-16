use super::*;

#[test]
fn execution_events_publish_authoritative_terminals_and_worker_failures() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let (client, server) = UnixStream::pair().unwrap();
    let writer = client_writer(server).unwrap();
    subscribe(&state, &writer).unwrap();
    let response = cwr::v2026_09::invocation::ActionResponse {
        progress: None,
        invocation_ref: "cwp://example.test/invocations/one".into(),
        sequence: 4,
        status: cwr::v2026_09::invocation::ActionResponseStatus::COMPLETED,
        summary: "Finished".into(),
        result: Some(serde_json::json!({"value": 42})),
        error: None,
        cause: None,
    };

    broadcast_execution_event(
        &state,
        "cwp://example.test/conversations/one",
        &LocalRuntimeEvent::Terminal {
            target_actor_ref: crate::test_refs::actor_ref("helper").as_str().into(),
            response: Box::new(response.clone()),
            message: None,
        },
    )
    .unwrap();
    broadcast_execution_failure(
        &state,
        "cwp://example.test/conversations/two",
        "cwp://example.test/invocations/two",
        crate::test_refs::actor_ref("helper").as_str(),
        "could not persist terminal state",
    )
    .unwrap();

    let events = BufReader::new(client)
        .lines()
        .take(2)
        .map(|line| serde_json::from_str::<RuntimeServiceEvent>(&line.unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        &events[0],
        RuntimeServiceEvent::ConversationExecutionFinished {
            response: observed,
            ..
        } if **observed == response
    ));
    assert!(matches!(
        &events[1],
        RuntimeServiceEvent::ConversationExecutionFailed { error, .. }
            if error == "could not persist terminal state"
    ));
}

#[test]
fn repeated_stop_publication_emits_one_completion_to_subscribers() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let (client, server) = UnixStream::pair().unwrap();
    let writer = client_writer(server).unwrap();
    subscribe(&state, &writer).unwrap();
    let response = cwr::v2026_09::invocation::ActionResponse {
        progress: None,
        invocation_ref: "cwp://example.test/invocations/stopped".into(),
        sequence: 1,
        status: cwr::v2026_09::invocation::ActionResponseStatus::STOPPED,
        summary: "Stopped".into(),
        result: None,
        error: None,
        cause: None,
    };
    // Worker fallback, acknowledgement and the scheduler may all race to publish.
    for _ in 0..3 {
        broadcast_execution_terminal(&state, "conversation", "actor", Box::new(response.clone()))
            .unwrap();
    }
    broadcast_execution_failure(&state, "conversation", "sentinel", "actor", "sentinel").unwrap();
    let events = wire::read_events(client, 2).join().unwrap();
    assert!(
        matches!(&events[0], RuntimeServiceEvent::ConversationExecutionFinished { response, .. }
        if response.invocation_ref == "cwp://example.test/invocations/stopped")
    );
    assert!(
        matches!(&events[1], RuntimeServiceEvent::ConversationExecutionFailed { invocation_ref, .. }
        if invocation_ref == "sentinel")
    );
}
