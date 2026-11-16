use super::*;

#[test]
fn restarting_peer_disconnect_preserves_the_owners_acknowledgement_and_notice() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let (owner_client, owner_server) = wire::wire_pair();
    let owner = client_writer(owner_server).unwrap();
    let (peer_client, peer_server) = wire::wire_pair();
    let peer = client_writer(peer_server).unwrap();
    subscribe(&state, &owner).unwrap();
    subscribe(&state, &peer).unwrap();

    // Keep the owner's output pending while another subscriber receives its
    // notice and disconnects. That cleanup broadcasts an updated catalog.
    write_event(
        &owner,
        &RuntimeServiceEvent::CommandFailed {
            request_id: "blocked-write".into(),
            error: "x".repeat(2 * 1024 * 1024),
        },
    )
    .unwrap();
    for operation in ["begin", "finish"] {
        execute_command(
            &state,
            &owner,
            RuntimeServiceCommand::DevGenerationRestart {
                version: RUNTIME_SERVICE_VERSION,
                request_id: operation.into(),
                operation: operation.into(),
                generation: "replacement".into(),
            },
        )
        .unwrap();
    }
    let events = wire::read_events(peer_client, 1).join().unwrap();
    assert!(matches!(
        events[0],
        RuntimeServiceEvent::RuntimeRestarting { .. }
    ));
    unsubscribe(&state, &peer);

    let events: Vec<RuntimeServiceEvent> = BufReader::new(owner_client)
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect();
    assert_eq!(
        events.len(),
        4,
        "pending restart output must drain before EOF"
    );
    for (event, expected) in events[1..3].iter().zip(["begin", "finish"]) {
        assert!(
            matches!(event, RuntimeServiceEvent::DrainStatus { request_id, remaining: 0 }
            if request_id == expected)
        );
    }
    assert!(
        matches!(&events[3], RuntimeServiceEvent::RuntimeRestarting { generation }
        if generation == "replacement")
    );
}

#[test]
fn service_stop_cancels_the_turn_and_clears_pending_status_for_connected_hosts() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let admission = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "stop-root",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@helper check this",
        )
        .unwrap();
    let invocation_ref = admission.invocation_ref().unwrap();
    let conversation_ref = admission.conversation_ref();
    assert!(!runtime.conversation_turn_statuses().unwrap().is_empty());

    let (client, server) = wire::wire_pair();
    let received = wire::read_events(client, 4);
    let writer = client_writer(server).unwrap();
    lock_state(&state)
        .unwrap()
        .subscribers
        .push(Arc::clone(&writer));
    execute_command(
        &state,
        &writer,
        RuntimeServiceCommand::StopConversation {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "slack-native-stop".into(),
            conversation_ref: conversation_ref.into(),
            requesting_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            invocation_ref: None,
            event_at_unix_ms: Some(current_unix_ms()),
        },
    )
    .unwrap();

    let events = received.join().unwrap();
    let response = &events[0];
    assert!(matches!(response, RuntimeServiceEvent::CommandSucceeded {
        request_id, stopped_invocation_refs, ..
    } if request_id == "slack-native-stop" && stopped_invocation_refs == &[invocation_ref]));
    assert!(matches!(
        events[1],
        RuntimeServiceEvent::ConversationUpsert { .. }
    ));
    assert!(events.iter().any(
        |event| matches!(event, RuntimeServiceEvent::ConversationTurnStatusSnapshot {
        conversation_ref: reference, turns,
    } if reference == conversation_ref && turns.is_empty())
    ));
    assert!(
        events.iter().any(|event| matches!(
            event,
            RuntimeServiceEvent::ConversationExecutionFinished { response, .. }
                if response.invocation_ref == invocation_ref
                    && response.status
                        == cwr::v2026_09::invocation::ActionResponseStatus::STOPPED
        )),
        "missing stop terminal: {events:?}"
    );
    assert_eq!(
        runtime
            .turn(&admission.record)
            .unwrap()
            .invocation
            .unwrap()
            .state,
        cwr::v2026_09::invocation::ActionInvocationState::Stopped
    );
}

#[test]
fn service_can_cancel_one_of_several_queued_turns_without_starting_it() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let first = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "first",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@helper first",
        )
        .unwrap();
    let queued = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "queued",
            crate::test_refs::actor_ref("person").as_str(),
            Some(first.conversation_ref()),
            "@helper queued",
        )
        .unwrap();
    let (client, server) = wire::wire_pair();
    let received = wire::read_events(client, 3);
    let writer = client_writer(server).unwrap();
    lock_state(&state)
        .unwrap()
        .subscribers
        .push(Arc::clone(&writer));
    execute_command(
        &state,
        &writer,
        RuntimeServiceCommand::StopConversation {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "desktop-cancel-queued".into(),
            conversation_ref: first.conversation_ref().into(),
            requesting_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            invocation_ref: queued.invocation_ref().map(str::to_owned),
            event_at_unix_ms: None,
        },
    )
    .unwrap();
    let events = received.join().unwrap();
    assert!(
        matches!(&events[0], RuntimeServiceEvent::CommandSucceeded { stopped_invocation_refs, .. }
        if stopped_invocation_refs == &[queued.invocation_ref().unwrap()])
    );
    assert!(
        matches!(&events[2], RuntimeServiceEvent::ConversationTurnStatusSnapshot { turns, .. }
        if turns.len() == 1 && turns[0].invocation_ref == first.invocation_ref().unwrap())
    );
    let stopped = runtime.turn(&queued.record).unwrap().invocation.unwrap();
    assert_eq!(stopped.attempt_count, 0);
    assert_eq!(
        stopped.state,
        cwr::v2026_09::invocation::ActionInvocationState::Stopped
    );
}

#[test]
fn upstream_commands_require_the_connections_selected_actor() {
    use cwr::v2026_09::upstream::{UpstreamCommand, UpstreamCoworker};
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let (_client, server) = wire::wire_pair();
    let writer = client_writer(server).unwrap();
    let command = UpstreamCommand::Register {
        coworkers: vec![UpstreamCoworker {
            actor_ref: "cwp://example.test/actors/chat".into(),
            coworker_ref: "cwp://example.test/coworkers/chat".into(),
            handle: "@chat".into(),
            name: "Chat".into(),
            actions: vec![],
        }],
    };
    assert!(execute_upstream_command(
        &state,
        &writer,
        "missing".into(),
        crate::test_refs::actor_ref("person").as_str().into(),
        command.clone()
    )
    .is_err());
    lock_state(&state).unwrap().host_actors.insert(
        upstream_owner(&writer),
        crate::test_refs::actor_ref("person").as_str().into(),
    );
    let error = execute_upstream_command(
        &state,
        &writer,
        "spoof".into(),
        crate::test_refs::actor_ref("helper").as_str().into(),
        command.clone(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("selected actor"));
    execute_upstream_command(
        &state,
        &writer,
        "valid".into(),
        crate::test_refs::actor_ref("person").as_str().into(),
        command,
    )
    .unwrap();
}

#[test]
fn host_local_catalog_is_not_broadcast_to_another_connection() {
    use cwr::v2026_09::upstream::UpstreamCoworker;

    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let actor = UpstreamCoworker {
        actor_ref: "cwp://example.test/actors/chat".into(),
        coworker_ref: "cwp://example.test/coworkers/chat".into(),
        handle: "@chat".into(),
        name: "Chat".into(),
        actions: Vec::new(),
    };
    let (_provider_client, provider_server) = wire::wire_pair();
    let provider = client_writer(provider_server).unwrap();
    let owner = upstream_owner(&provider);
    let session = runtime
        .register_upstream_provider(
            &owner,
            vec![actor.clone()],
            Arc::new(|_| Ok(())),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    runtime
        .enable_upstream_coworker(
            &owner,
            &session,
            &actor.actor_ref,
            crate::test_refs::actor_ref("person").as_str(),
            true,
        )
        .unwrap();
    assert!(runtime
        .upstream_catalog_as(crate::test_refs::actor_ref("person").as_str(), Some(&owner))
        .unwrap()
        .actor_by_handle("@chat")
        .is_some());

    let (participant_client, participant_server) = wire::wire_pair();
    let participant = client_writer(participant_server).unwrap();
    lock_state(&state).unwrap().host_actors.insert(
        upstream_owner(&participant),
        crate::test_refs::actor_ref("person").as_str().into(),
    );
    lock_state(&state)
        .unwrap()
        .subscribers
        .push(Arc::clone(&participant));
    let received = wire::read_events(participant_client, 1);

    broadcast(
        &state,
        RuntimeServiceEvent::CatalogUpdated {
            catalog: runtime.catalog().unwrap(),
        },
    )
    .unwrap();

    let events = received.join().unwrap();
    let RuntimeServiceEvent::CatalogUpdated { catalog } = &events[0] else {
        panic!("expected a scoped catalog update")
    };
    assert!(catalog.actor_by_handle("@chat").is_none());
}

#[test]
fn person_reply_rejects_an_actor_not_bound_to_the_connection() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let (_client, server) = wire::wire_pair();
    let writer = client_writer(server).unwrap();
    let error = execute_command(
        &state,
        &writer,
        RuntimeServiceCommand::ReplyPersonRequest {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "reply".into(),
            actor_ref: crate::test_refs::actor_ref("person"),
            conversation_ref: "cwp://test/conversations/one".into(),
            invocation_ref: "cwp://test/invocations/one".into(),
            text: "answer".into(),
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("connection's selected actor"));
}
