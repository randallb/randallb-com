use super::*;
use std::time::Instant;

fn payload(bytes: usize) -> RuntimeServiceEvent {
    RuntimeServiceEvent::CommandFailed {
        request_id: "output-diagnostics".into(),
        error: "x".repeat(bytes),
    }
}

#[test]
fn queue_count_retirement_is_distinct_from_the_byte_budget() {
    let (_client, server) = UnixStream::pair().unwrap();
    let writer = client_writer(server).unwrap();
    // One frame holds the socket writer; small later batches fill only the
    // queue-count limit, with plenty of byte budget remaining.
    write_event(&writer, &payload(2 * 1024 * 1024)).unwrap();
    for _ in 0..=OUTPUT_QUEUE_CAPACITY {
        if write_event(&writer, &payload(1)).is_err() {
            break;
        }
    }
    assert_eq!(writer.state.failure.get(), Some(&"queue_count"));
    assert!(writer.queue.pending.lock().unwrap().budgeted_bytes < OUTPUT_QUEUE_BYTE_CAPACITY);
    disconnect_writer(&writer);
}

#[test]
fn byte_budget_retirement_keeps_its_reason_after_later_writes() {
    let (_client, server) = UnixStream::pair().unwrap();
    let writer = client_writer(server).unwrap();
    let event = payload(OUTPUT_QUEUE_BYTE_CAPACITY / 3);
    write_event(&writer, &event).unwrap();
    write_event(&writer, &event).unwrap();
    assert!(write_event(&writer, &event).is_err());
    assert_eq!(writer.state.failure.get(), Some(&"byte_budget"));
    assert!(write_event(&writer, &payload(1)).is_err());
    assert_eq!(writer.state.failure.get(), Some(&"byte_budget"));
    disconnect_writer(&writer);
}

#[test]
fn socket_write_timeout_retains_its_own_reason() {
    let (_client, server) = UnixStream::pair().unwrap();
    let writer = client_writer_with_timeout(server, Duration::from_millis(25)).unwrap();
    write_event(&writer, &payload(2 * 1024 * 1024)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while writer.state.failure.get().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(writer.state.failure.get(), Some(&"socket_write"));
    assert!(!writer_is_connected(&writer));
    disconnect_writer(&writer);
}

fn read_reply(request_id: &str, bytes: usize) -> RuntimeServiceEvent {
    serde_json::from_value(serde_json::json!({
        "kind": "conversation_records_page", "request_id": request_id,
        "page": {"kind": "content", "page": {
            "content": [{"key": {"position": 0}, "message": {
                "message_ref": "cwp://example.test/messages/one",
                "author_actor_ref": "cwp://example.test/actors/person",
                "addressed_actor_refs": [], "text": "x".repeat(bytes)
            }}], "has_next_page": false, "has_previous_page": false,
            "total_count": 1, "revision": "1"
        }}
    }))
    .unwrap()
}

#[test]
fn concurrent_read_replies_wait_for_capacity_without_retiring_the_connection() {
    use std::io::{BufRead, BufReader};
    let (client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let writer = client_writer(server).unwrap();
    let first = read_reply("one", OUTPUT_QUEUE_BYTE_CAPACITY / 3);
    let second = read_reply("two", OUTPUT_QUEUE_BYTE_CAPACITY / 3);
    let third = read_reply("three", OUTPUT_QUEUE_BYTE_CAPACITY / 3);
    let deadline = Instant::now() + Duration::from_secs(10);
    write_read_event(&writer, &first, deadline).unwrap();
    write_read_event(&writer, &second, deadline).unwrap();
    let writing_writer = Arc::clone(&writer);
    let (finished, completion) = std::sync::mpsc::channel();
    let writing = thread::spawn(move || {
        finished
            .send(write_read_event(&writing_writer, &third, deadline).is_ok())
            .unwrap()
    });
    assert!(
        completion.recv_timeout(Duration::from_secs(1)).is_err(),
        "read reply must wait for output space"
    );
    assert!(writer_is_connected(&writer));
    assert!(writer.state.failure.get().is_none());
    assert!(writer.queue.pending.lock().unwrap().budgeted_bytes <= OUTPUT_QUEUE_BYTE_CAPACITY);
    // Control/publication stays nonblocking while the read worker waits.
    let control = payload(1);
    write_event(&writer, &control).unwrap();
    let events: Vec<RuntimeServiceEvent> = BufReader::new(client)
        .lines()
        .take(4)
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect();
    assert_eq!(events[0], first);
    assert_eq!(events[1], second);
    assert_eq!(events[2], control);
    assert!(
        matches!(&events[3], RuntimeServiceEvent::ConversationRecordsPage { request_id, .. } if request_id == "three")
    );
    assert!(completion.recv_timeout(Duration::from_secs(1)).unwrap());
    writing.join().unwrap();
    assert!(writer_is_connected(&writer));
    assert!(writer.state.failure.get().is_none());
    disconnect_writer(&writer);
}

#[test]
fn read_output_deadline_fails_only_the_request() {
    use std::io::{BufRead, BufReader};
    let (client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let writer = client_writer(server).unwrap();
    let first = read_reply("one", OUTPUT_QUEUE_BYTE_CAPACITY / 3);
    let second = read_reply("two", OUTPUT_QUEUE_BYTE_CAPACITY / 3);
    let deadline = Instant::now() + Duration::from_secs(5);
    write_read_event(&writer, &first, deadline).unwrap();
    write_read_event(&writer, &second, deadline).unwrap();
    let error = write_read_event(
        &writer,
        &read_reply("expired", OUTPUT_QUEUE_BYTE_CAPACITY / 3),
        Instant::now() + Duration::from_millis(25),
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::TimedOut
    );
    assert!(writer_is_connected(&writer));
    assert!(writer.state.failure.get().is_none());
    let events: Vec<RuntimeServiceEvent> = BufReader::new(client)
        .lines()
        .take(3)
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect();
    assert_eq!(events[0], first);
    assert_eq!(events[1], second);
    assert!(
        matches!(&events[2], RuntimeServiceEvent::CommandFailed { request_id, error }
        if request_id == "expired" && error.contains("deadline"))
    );
    disconnect_writer(&writer);
}

#[test]
fn read_production_permit_bounds_materialized_replies_and_honors_cancellation() {
    let (_client, server) = UnixStream::pair().unwrap();
    let writer = client_writer(server).unwrap();
    let guard =
        read_reply_guard(&writer, "first", Instant::now() + Duration::from_secs(1)).unwrap();
    let second_writer = Arc::clone(&writer);
    let waiting = thread::spawn(move || {
        read_reply_guard(
            &second_writer,
            "second",
            Instant::now() + Duration::from_millis(25),
        )
        .err()
        .unwrap()
        .kind()
    });
    assert_eq!(waiting.join().unwrap(), io::ErrorKind::TimedOut);
    cancel_read(&writer, "cancelled");
    assert_eq!(
        read_reply_guard(
            &writer,
            "cancelled",
            Instant::now() + Duration::from_secs(1)
        )
        .err()
        .unwrap()
        .kind(),
        io::ErrorKind::Interrupted
    );
    drop(guard);
    drop(read_reply_guard(&writer, "next", Instant::now() + Duration::from_secs(1)).unwrap());
    assert!(writer_is_connected(&writer));
    disconnect_writer(&writer);
}
