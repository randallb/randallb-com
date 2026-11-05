use super::wire::{read_events, wire_pair};
use super::*;
use std::io::{BufRead, Write};

#[test]
fn desktop_and_full_history_subscribers_receive_projected_updates() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let (desktop_client, desktop_server) = wire_pair();
    let desktop_writer = client_writer(desktop_server).expect("create Desktop event writer");
    writer::enable_metadata_bootstrap(&desktop_writer);
    writer::enable_fork_metadata(&desktop_writer);
    subscribe(&state, &desktop_writer).expect("subscribe Desktop client");
    let (legacy_client, legacy_server) = wire_pair();
    let legacy_writer = client_writer(legacy_server).expect("create legacy event writer");
    writer::enable_metadata_bootstrap(&legacy_writer);
    subscribe(&state, &legacy_writer).expect("subscribe legacy client");
    let (full_client, full_server) = wire_pair();
    let full_writer = client_writer(full_server).expect("create full-history event writer");
    subscribe(&state, &full_writer).expect("subscribe full-history client");
    assert!(needs_full_conversation_broadcast(&state).unwrap());
    let message = cwr::v2026_09::conversation::ConversationMessage {
        file_refs: Vec::new(),
        progress: None,
        message_ref: "cwp://example.test/messages/one".into(),
        author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        addressed_actor_refs: Vec::new(),
        addressing_mode: Default::default(),
        text: format!("Readable title\n{}", "x".repeat(256)),
        visualizations: Vec::new(),
        reply_to_message_ref: None,
        action: None,
    };
    let lineage = ConversationActivity {
        activity_ref: "cwp://example.test/activities/lineage".into(),
        sequence: 1,
        occurred_at_unix_ms: 1,
        event: ConversationActivityEvent::ConversationForkedFrom {
            inherited_refs: Default::default(),
            source_conversation_ref: "cwp://example.test/conversations/source".into(),
            source_message_ref: "cwp://example.test/messages/source".into(),
            forked_by_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            operation_id: "fork-operation".into(),
            forked_at_unix_ms: 1,
            lineage_depth: 1,
        },
    };
    let event = RuntimeServiceEvent::ConversationUpsert {
        conversation: cwr::v2026_09::conversation::RetainedConversation {
            owner_context_ref: None,
            conversation_ref: "cwp://example.test/conversations/large".into(),
            archived: false,
            title: None,
            sidebar_eligible: None,
            parent_conversation_ref: None,
            participant_actor_refs: vec![crate::test_refs::actor_ref("person").as_str().into()],
            messages: vec![message; 2_000],
            timeline_connection_ref: None,
            activity: vec![lineage],
        },
    };
    let full_bytes = serde_json::to_vec(&event).unwrap().len();
    let RuntimeServiceEvent::ConversationUpsert { conversation } = &event else {
        unreachable!()
    };
    let mut legacy_full_snapshot = vec![conversation.clone()];
    snapshot::strip_fork_metadata(&mut legacy_full_snapshot);
    assert!(
        !legacy_full_snapshot[0].activity.iter().any(|item| matches!(
            item.event,
            ConversationActivityEvent::ConversationForkedFrom { .. }
                | ConversationActivityEvent::MessageWorkAssociation { .. }
        ))
    );

    broadcast(&state, event).expect("broadcast projected update");
    let desktop_received = read_events(desktop_client, 1)
        .join()
        .expect("read Desktop update");
    let full_received = read_events(full_client, 1)
        .join()
        .expect("read full-history update");
    let legacy_received = read_events(legacy_client, 1)
        .join()
        .expect("read legacy update");
    let desktop_bytes = serde_json::to_vec(&desktop_received[0]).unwrap().len();
    let RuntimeServiceEvent::ConversationUpsert { conversation } = &desktop_received[0] else {
        panic!("expected conversation update")
    };
    assert_eq!(conversation.title.as_deref(), Some("Readable title"));
    assert!(conversation.messages.is_empty());
    assert!(conversation.activity.iter().any(|item| matches!(
        item.event,
        ConversationActivityEvent::ConversationForkedFrom { .. }
    )));
    let RuntimeServiceEvent::ConversationUpsert { conversation } = &full_received[0] else {
        panic!("expected full-history conversation update")
    };
    assert_eq!(conversation.messages.len(), 2_000);
    assert!(!conversation.activity.iter().any(|item| matches!(
        item.event,
        ConversationActivityEvent::ConversationForkedFrom { .. }
    )));
    let RuntimeServiceEvent::ConversationUpsert { conversation } = &legacy_received[0] else {
        panic!("expected legacy conversation update")
    };
    assert!(conversation.messages.is_empty());
    assert!(!conversation.activity.iter().any(|item| matches!(
        item.event,
        ConversationActivityEvent::ConversationForkedFrom { .. }
    )));
    assert!(
        full_bytes > desktop_bytes * 1_000,
        "{full_bytes} <= {desktop_bytes} * 1000"
    );
    unsubscribe(&state, &full_writer);
    assert!(!needs_full_conversation_broadcast(&state).unwrap());
}

#[test]
fn identity_and_transfer_probes_preserve_desktop_snapshot_bootstrap() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let (mut command_client, command_server) = UnixStream::pair().unwrap();
    let (event_client, event_server) = wire_pair();
    let writer = client_writer(event_server).unwrap();
    let connection_state = Arc::clone(&state);
    let connection_writer = Arc::clone(&writer);
    let connection = std::thread::spawn(move || {
        handle_connection_commands(&connection_state, &connection_writer, command_server)
    });
    let mut events = BufReader::new(event_client).lines();

    serde_json::to_writer(
        &mut command_client,
        &RuntimeServiceCommand::Drain {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "identity-probe".into(),
            operation: "supports-build-identity".into(),
        },
    )
    .unwrap();
    command_client.write_all(b"\n").unwrap();
    assert!(matches!(
        serde_json::from_str::<RuntimeServiceEvent>(&events.next().unwrap().unwrap()).unwrap(),
        RuntimeServiceEvent::BuildIdentity { request_id, .. } if request_id == "identity-probe"
    ));
    assert!(lock_state(&state).unwrap().subscribers.is_empty());

    serde_json::to_writer(
        &mut command_client,
        &RuntimeServiceCommand::Drain {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "transfer-probe".into(),
            operation: "supports-conversation-transfer".into(),
        },
    )
    .unwrap();
    command_client.write_all(b"\n").unwrap();
    assert!(matches!(
        serde_json::from_str::<RuntimeServiceEvent>(&events.next().unwrap().unwrap()).unwrap(),
        RuntimeServiceEvent::DrainStatus { request_id, .. } if request_id == "transfer-probe"
    ));
    assert!(lock_state(&state).unwrap().subscribers.is_empty());

    serde_json::to_writer(
        &mut command_client,
        &RuntimeServiceCommand::Drain {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "observation-probe".into(),
            operation: "supports-resident-observation-status".into(),
        },
    )
    .unwrap();
    command_client.write_all(b"\n").unwrap();
    assert!(matches!(
        serde_json::from_str::<RuntimeServiceEvent>(&events.next().unwrap().unwrap()).unwrap(),
        RuntimeServiceEvent::DrainStatus { request_id, remaining: 0 } if request_id == "observation-probe"
    ));
    assert!(lock_state(&state).unwrap().subscribers.is_empty());

    serde_json::to_writer(
        &mut command_client,
        &RuntimeServiceCommand::Snapshot {
            version: RUNTIME_SERVICE_VERSION,
            request_id: DESKTOP_METADATA_SNAPSHOT_REQUEST_ID.into(),
            metadata_only: true,
        },
    )
    .unwrap();
    command_client.write_all(b"\n").unwrap();
    while !matches!(
        serde_json::from_str::<RuntimeServiceEvent>(&events.next().unwrap().unwrap()).unwrap(),
        RuntimeServiceEvent::CommandSucceeded { request_id, .. }
            if request_id == DESKTOP_METADATA_SNAPSHOT_REQUEST_ID
    ) {}
    // The event writer can deliver CommandSucceeded before snapshot handling
    // registers the subscriber. Wait for the command loop to finish, rather
    // than racing its final state update after reading that event.
    command_client.shutdown(std::net::Shutdown::Write).unwrap();
    connection.join().unwrap().unwrap();
    assert_eq!(lock_state(&state).unwrap().subscribers.len(), 1);
    assert!(writer::uses_metadata_bootstrap(&writer));
    assert!(writer::includes_fork_metadata(&writer));

    unsubscribe(&state, &writer);
}
