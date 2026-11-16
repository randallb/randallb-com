use super::writer::try_write_event;
use super::*;
use std::io::{self, Read, Write};

#[test]
fn resident_rejects_an_incomplete_command_frame() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let (mut client, server) = UnixStream::pair().expect("create command wire pair");
    let (_event_client, event_server) = UnixStream::pair().expect("create event wire pair");
    let writer = client_writer(event_server).expect("create event writer");
    client
        .write_all(br#"{"kind":"snapshot","version":"cwr.runtime-service/1""#)
        .expect("write partial command");
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("end partial command");

    let error = handle_connection_commands(&state, &writer, server)
        .expect_err("resident must reject a command without a newline");

    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::UnexpectedEof
    );
    assert!(error.to_string().contains("incomplete JSONL frame"));
}

pub(super) fn wire_pair() -> (UnixStream, UnixStream) {
    let (client, server) = UnixStream::pair().unwrap();
    let timeout = Some(std::time::Duration::from_secs(3));
    client.set_read_timeout(timeout).unwrap();
    server.set_write_timeout(timeout).unwrap();
    (client, server)
}

pub(super) fn read_events(
    client: UnixStream,
    count: usize,
) -> std::thread::JoinHandle<Vec<RuntimeServiceEvent>> {
    std::thread::spawn(move || {
        let mut lines = BufReader::new(client).lines();
        (0..count)
            .map(|_| serde_json::from_str(&lines.next().expect("runtime event").unwrap()).unwrap())
            .collect()
    })
}

#[test]
fn small_snapshot_with_many_fields_does_not_exhaust_the_socket_buffer() {
    let conversations = (0..24)
        .map(|i| cwr::v2026_09::conversation::RetainedConversation {
            owner_context_ref: None,
            conversation_ref: format!("cwp://example.test/conversations/{i}"),
            title: None,
            archived: false,
            sidebar_eligible: None,
            parent_conversation_ref: None,
            participant_actor_refs: Vec::new(),
            messages: Vec::new(),
            timeline_connection_ref: None,
            activity: Vec::new(),
        })
        .collect();
    let event = RuntimeServiceEvent::ConversationSnapshot { conversations };
    assert!(serde_json::to_vec(&event).unwrap().len() < 4096);
    let (client, server) = wire_pair();
    // Deliberately leave the reader idle: this small frame fits, but hundreds
    // of individual serialization writes can fill Linux's socket buffer.
    write_event(&client_writer(server).unwrap(), &event).unwrap();
    let mut lines = BufReader::new(client).lines();
    let received: RuntimeServiceEvent =
        serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(received).unwrap(),
        serde_json::to_value(event).unwrap()
    );
}

#[test]
fn runtime_client_output_allows_a_delayed_consumer() {
    let (client, server) = UnixStream::pair().expect("create runtime wire pair");
    let socket_options = server.try_clone().expect("inspect writer socket options");
    let writer = client_writer(server).expect("create event writer");

    assert_eq!(
        socket_options.write_timeout().unwrap(),
        Some(std::time::Duration::from_secs(30))
    );

    let event = RuntimeServiceEvent::CommandFailed {
        request_id: "delayed-consumer".into(),
        error: "x".repeat(2 * 1024 * 1024),
    };
    try_write_event(&writer, &event).expect("enqueue large event");
    std::thread::sleep(std::time::Duration::from_millis(50));

    let received = read_events(client, 1).join().expect("read complete event");
    assert_eq!(received.len(), 1);
    assert_eq!(
        serde_json::to_value(&received[0]).unwrap(),
        serde_json::to_value(event).unwrap()
    );
    disconnect_writer(&writer);
}

#[test]
fn queue_exhaustion_retires_after_a_complete_frame() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let (mut client, server) = UnixStream::pair().expect("create runtime wire pair");
    let writer = client_writer(server).expect("create event writer");
    subscribe(&state, &writer).expect("subscribe client");
    let event = RuntimeServiceEvent::CommandFailed {
        request_id: "retired-slow-consumer".into(),
        error: "x".repeat(2 * 1024 * 1024),
    };

    while write_event(&writer, &event).is_ok() {}
    unsubscribe(&state, &writer);

    let mut output = Vec::new();
    client
        .read_to_end(&mut output)
        .expect("read complete frames before retirement");
    assert_eq!(output.last(), Some(&b'\n'));
    let lines = output.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    assert!(lines.len() > 1);
    assert!(lines.last().unwrap().is_empty());
    for line in &lines[..lines.len() - 1] {
        let received: RuntimeServiceEvent =
            serde_json::from_slice(line).expect("retirement must not truncate a frame");
        assert_eq!(
            serde_json::to_value(received).unwrap(),
            serde_json::to_value(&event).unwrap()
        );
    }
}

#[test]
fn retiring_an_idle_writer_closes_its_socket() {
    let (mut client, server) = UnixStream::pair().expect("create runtime wire pair");
    let writer = client_writer(server).expect("create event writer");

    writer::retire_writer(&writer);

    let mut output = Vec::new();
    client
        .read_to_end(&mut output)
        .expect("idle writer retirement must close its socket");
    assert!(output.is_empty());
}

#[test]
fn disconnect_retires_the_writer_before_server_cleanup_locks() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let held_state = lock_state(&state).expect("hold runtime state");
    let (_client, server) = UnixStream::pair().expect("create runtime wire pair");
    let writer = client_writer(server).expect("create event writer");
    let cleanup_state = Arc::clone(&state);
    let cleanup_writer = Arc::clone(&writer);
    let cleanup = std::thread::spawn(move || unsubscribe(&cleanup_state, &cleanup_writer));

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
    while writer::writer_is_connected(&writer) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(!writer::writer_is_connected(&writer));
    drop(held_state);
    cleanup.join().expect("finish disconnected client cleanup");
}

#[test]
fn stalled_oversized_output_releases_its_writer() {
    let (_client, server) = UnixStream::pair().expect("create runtime wire pair");
    let writer =
        writer::client_writer_with_test_timeout(server, std::time::Duration::from_millis(25))
            .expect("create event writer");
    let event = RuntimeServiceEvent::CommandFailed {
        request_id: "stalled-oversized-output".into(),
        error: "x".repeat(writer::OUTPUT_QUEUE_BYTE_CAPACITY + 1),
    };

    try_write_event(&writer, &event).expect("enqueue oversized output");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while writer::writer_is_connected(&writer) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert!(!writer::writer_is_connected(&writer));
}

#[test]
fn initial_snapshot_finishes_before_the_client_receives_live_events() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let (client, server) = UnixStream::pair().expect("create runtime wire pair");
    let writer = client_writer(server).unwrap();

    snapshot::write_initial_runtime_snapshot(&state, &writer, "initial-snapshot".into())
        .expect("write and subscribe initial snapshot");
    broadcast(
        &state,
        RuntimeServiceEvent::ConversationSnapshot {
            conversations: Vec::new(),
        },
    )
    .expect("broadcast after initial snapshot");

    let events = BufReader::new(client)
        .lines()
        .take(3)
        .map(|line| serde_json::from_str::<RuntimeServiceEvent>(&line.unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        events.as_slice(),
        [
            RuntimeServiceEvent::ConversationSnapshot { .. },
            RuntimeServiceEvent::CommandSucceeded { request_id, .. },
            RuntimeServiceEvent::ConversationSnapshot { .. },
        ] if request_id == "initial-snapshot"
    ));
    assert_eq!(lock_state(&state).unwrap().subscribers.len(), 1);
}

#[test]
fn failed_broadcast_prunes_a_disconnected_subscriber() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let (client, server) = UnixStream::pair().expect("create runtime wire pair");
    let writer = client_writer(server).unwrap();
    subscribe(&state, &writer).expect("subscribe client");
    drop(client);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !lock_state(&state).unwrap().subscribers.is_empty()
        && std::time::Instant::now() < deadline
    {
        broadcast(
            &state,
            RuntimeServiceEvent::ConversationSnapshot {
                conversations: Vec::new(),
            },
        )
        .expect("broadcast tolerates disconnected clients");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert!(lock_state(&state).unwrap().subscribers.is_empty());
}

#[test]
fn bursts_within_the_queue_budget_preserve_complete_json_frames() {
    const EVENT_COUNT: usize = 200;
    const PAYLOAD_BYTES: usize = 16 * 1024;

    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let (client, server) = wire_pair();
    let writer = client_writer(server).unwrap();
    subscribe(&state, &writer).expect("subscribe client");

    let broadcast_state = Arc::clone(&state);
    let broadcaster = std::thread::spawn(move || {
        for index in 0..EVENT_COUNT {
            broadcast(
                &broadcast_state,
                RuntimeServiceEvent::CommandFailed {
                    request_id: format!("burst-{index}"),
                    error: "x".repeat(PAYLOAD_BYTES),
                },
            )
            .expect("broadcast event");
        }
    });

    std::thread::sleep(std::time::Duration::from_millis(50));
    let events = read_events(client, EVENT_COUNT);
    broadcaster.join().expect("finish burst broadcasts");
    let received = events.join().expect("read complete JSON frames");

    assert_eq!(received.len(), EVENT_COUNT);
    assert!(received.iter().enumerate().all(|(index, event)| matches!(
        event,
        RuntimeServiceEvent::CommandFailed { request_id, error }
            if request_id == &format!("burst-{index}") && error.len() == PAYLOAD_BYTES
    )));
    assert_eq!(lock_state(&state).unwrap().subscribers.len(), 1);
    disconnect_writer(&writer);
}

#[test]
fn slow_subscriber_is_disconnected_without_blocking_other_publications() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let (_slow_client, slow_server) = wire_pair();
    let slow_writer = client_writer(slow_server).unwrap();
    subscribe(&state, &slow_writer).unwrap();
    let event = RuntimeServiceEvent::CommandFailed {
        request_id: "slow-consumer".into(),
        error: "x".repeat(2 * 1024 * 1024),
    };
    while try_write_event(&slow_writer, &event).is_ok() {}

    let (healthy_client, healthy_server) = wire_pair();
    let healthy_writer = client_writer(healthy_server).unwrap();
    subscribe(&state, &healthy_writer).unwrap();
    let received = read_events(healthy_client, 1);
    let (completed, completion) = std::sync::mpsc::channel();
    let publishing_state = Arc::clone(&state);
    let publisher = std::thread::spawn(move || {
        completed.send(broadcast(&publishing_state, event)).unwrap();
    });
    let result = completion.recv_timeout(std::time::Duration::from_secs(1));
    // Also release a regressed blocking writer so a failed test cannot hang.
    disconnect_writer(&slow_writer);
    result
        .expect("slow client must not hold publication order")
        .unwrap();
    publisher.join().unwrap();
    assert_eq!(received.join().unwrap().len(), 1);
    assert_eq!(lock_state(&state).unwrap().subscribers.len(), 1);
    assert!(Arc::ptr_eq(
        &lock_state(&state).unwrap().subscribers[0],
        &healthy_writer
    ));
    disconnect_writer(&healthy_writer);
}

#[test]
fn concurrent_broadcasts_preserve_one_order_for_every_subscriber() {
    const EVENT_COUNT: usize = 24;

    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let (first_client, first_server) = wire_pair();
    let first_writer = client_writer(first_server).unwrap();
    subscribe(&state, &first_writer).expect("subscribe first client");
    let (second_client, second_server) = wire_pair();
    let second_writer = client_writer(second_server).unwrap();
    subscribe(&state, &second_writer).expect("subscribe second client");

    let first_events = read_events(first_client, EVENT_COUNT);
    let second_events = read_events(second_client, EVENT_COUNT);
    let starting_line = Arc::new(std::sync::Barrier::new(EVENT_COUNT + 1));
    let broadcasters = (0..EVENT_COUNT)
        .map(|index| {
            let state = Arc::clone(&state);
            let starting_line = Arc::clone(&starting_line);
            std::thread::spawn(move || {
                starting_line.wait();
                broadcast(
                    &state,
                    RuntimeServiceEvent::CommandFailed {
                        request_id: format!("concurrent-{index}"),
                        error: "expected test event".into(),
                    },
                )
                .expect("broadcast concurrent event");
            })
        })
        .collect::<Vec<_>>();
    starting_line.wait();
    for broadcaster in broadcasters {
        broadcaster.join().expect("finish concurrent broadcast");
    }

    let request_ids = |events: Vec<RuntimeServiceEvent>| {
        events
            .into_iter()
            .map(|event| match event {
                RuntimeServiceEvent::CommandFailed { request_id, .. } => request_id,
                other => panic!("expected command failure, received {other:?}"),
            })
            .collect::<Vec<_>>()
    };
    let first_request_ids = request_ids(first_events.join().expect("read first client events"));
    let second_request_ids = request_ids(second_events.join().expect("read second client events"));

    assert_eq!(first_request_ids, second_request_ids);
    disconnect_writer(&first_writer);
    disconnect_writer(&second_writer);
}

#[test]
fn subscribed_snapshots_wait_for_earlier_publications() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let (client, server) = wire_pair();
    let writer = client_writer(server).unwrap();
    subscribe(&state, &writer).expect("subscribe client");

    let publication = lock_broadcast(&state).expect("hold publication order");
    let snapshot_state = Arc::clone(&state);
    let snapshot_writer = Arc::clone(&writer);
    let (started, start) = std::sync::mpsc::channel();
    let (completed, completion) = std::sync::mpsc::channel();
    let snapshot = std::thread::spawn(move || {
        started.send(()).unwrap();
        let result = snapshot::write_runtime_snapshot(
            &snapshot_state,
            &snapshot_writer,
            "subscribed-snapshot".into(),
        );
        completed.send(result).unwrap();
    });
    start.recv().unwrap();
    assert!(completion
        .recv_timeout(std::time::Duration::from_millis(25))
        .is_err());

    write_event(
        &writer,
        &RuntimeServiceEvent::CommandFailed {
            request_id: "earlier-publication".into(),
            error: "expected test event".into(),
        },
    )
    .expect("enqueue earlier publication");
    drop(publication);
    completion
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("snapshot completes after publication")
        .expect("write subscribed snapshot");
    snapshot.join().unwrap();

    let events = BufReader::new(client)
        .lines()
        .take(2)
        .map(|line| serde_json::from_str::<RuntimeServiceEvent>(&line.unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        events.as_slice(),
        [
            RuntimeServiceEvent::CommandFailed { request_id, .. },
            RuntimeServiceEvent::ConversationSnapshot { .. },
        ] if request_id == "earlier-publication"
    ));
    disconnect_writer(&writer);
}

#[test]
fn blocked_snapshot_client_does_not_hold_the_runtime_state_mutex() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let (client, server) = UnixStream::pair().expect("create runtime wire pair");
    let writer = client_writer(server).unwrap();
    write_event(
        &writer,
        &RuntimeServiceEvent::CommandFailed {
            request_id: "fill-socket".into(),
            error: "x".repeat(2 * 1024 * 1024),
        },
    )
    .expect("enqueue a frame larger than the socket buffer");

    let snapshot_state = Arc::clone(&state);
    let snapshot_writer = Arc::clone(&writer);
    let (completed, completion) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = snapshot::write_initial_runtime_snapshot(
            &snapshot_state,
            &snapshot_writer,
            "nonblocking-snapshot".into(),
        );
        let _ = completed.send(result);
    });

    completion
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("snapshot enqueue must not wait for socket drainage")
        .expect("enqueue snapshot");
    assert!(state.state.try_lock().is_ok());
    drop(client);
    disconnect_writer(&writer);
}

#[test]
fn blocked_client_output_is_bounded_by_serialized_bytes() {
    let (client, server) = UnixStream::pair().expect("create runtime wire pair");
    let writer = client_writer(server).unwrap();
    let event = RuntimeServiceEvent::CommandFailed {
        request_id: "large-output".into(),
        error: "x".repeat(writer::OUTPUT_QUEUE_BYTE_CAPACITY / 3),
    };

    try_write_event(&writer, &event).expect("enqueue first large output");
    try_write_event(&writer, &event).expect("enqueue second large output");
    let error = try_write_event(&writer, &event).expect_err("byte budget must reject retention");

    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(error.to_string().contains("byte budget"));
    drop(client);
    disconnect_writer(&writer);
}

#[test]
fn one_oversized_batch_is_allowed_without_unbounded_queue_retention() {
    let (client, server) = UnixStream::pair().expect("create runtime wire pair");
    let writer = client_writer(server).unwrap();
    try_write_event(
        &writer,
        &RuntimeServiceEvent::Ready {
            runtime_ref: "cwp://example.test/runtime".into(),
        },
    )
    .expect("enqueue ordinary output before an oversized snapshot or response");
    let oversized = RuntimeServiceEvent::CommandFailed {
        request_id: "oversized-output".into(),
        error: "x".repeat(writer::OUTPUT_QUEUE_BYTE_CAPACITY + 1),
    };

    try_write_event(&writer, &oversized).expect("allow one unavoidable oversized batch");
    let error = try_write_event(&writer, &oversized)
        .expect_err("a second oversized batch must not be retained");

    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(client);
    disconnect_writer(&writer);
}
