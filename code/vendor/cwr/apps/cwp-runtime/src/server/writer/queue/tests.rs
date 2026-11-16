use super::*;
use serde_json::json;

fn upsert(conversation: &str, text: &str) -> RuntimeServiceEvent {
    serde_json::from_value(json!({
        "kind": "conversation_upsert", "conversation": {
            "conversation_ref": conversation, "participant_actor_refs": [],
            "messages": [{"message_ref": "message", "author_actor_ref": "person",
                "addressed_actor_refs": [], "text": text}], "activity": []
        }
    }))
    .unwrap()
}

fn push(queue: &OutputQueue, event: &RuntimeServiceEvent) {
    queue
        .push(encode_events(std::slice::from_ref(event)).unwrap())
        .unwrap();
}

fn drain(queue: &OutputQueue) -> Vec<RuntimeServiceEvent> {
    queue.close();
    let mut events = Vec::new();
    while let Some(batch) = queue.next() {
        events.extend(
            batch
                .frames
                .iter()
                .map(|frame| serde_json::from_slice::<RuntimeServiceEvent>(frame).unwrap()),
        );
        queue.complete(&batch);
    }
    assert_eq!(queue.pending.lock().unwrap().budgeted_bytes, 0);
    events
}

#[test]
fn growing_history_replaces_only_unsent_state_and_keeps_memory_flat() {
    let queue = OutputQueue::default();
    let first = upsert("one", &"x".repeat(256 * 1024));
    push(&queue, &first);
    let in_flight = queue.next().unwrap();
    let mut latest = first.clone();
    for revision in 1..=500 {
        latest = upsert("one", &"x".repeat(256 * 1024 + revision * 1024));
        push(&queue, &latest);
        let pending = queue.pending.lock().unwrap();
        assert_eq!(pending.batches.len(), 1);
        assert!(pending.budgeted_bytes < 2 * 1024 * 1024);
    }
    assert_eq!(
        serde_json::from_slice::<RuntimeServiceEvent>(&in_flight.frames[0]).unwrap(),
        first
    );
    queue.complete(&in_flight);
    assert_eq!(drain(&queue), vec![latest]);
}

#[test]
fn replacement_is_per_conversation_and_preserves_surviving_event_order() {
    let queue = OutputQueue::default();
    push(&queue, &upsert("one", "old"));
    push(&queue, &upsert("two", "other"));
    push(&queue, &upsert("one", "new"));
    assert_eq!(
        drain(&queue),
        vec![upsert("two", "other"), upsert("one", "new")]
    );
}

#[test]
fn control_and_lifecycle_events_are_coalescing_barriers() {
    let barriers = [
        json!({"kind":"conversation_snapshot", "conversations": []}),
        json!({"kind":"command_succeeded", "request_id":"receipt", "stopped_invocation_refs":["work"]}),
        json!({"kind":"command_failed", "request_id":"failed", "error":"failure"}),
        json!({"kind":"conversation_execution_finished", "conversation_ref":"one", "target_actor_ref":"helper", "response":{
            "invocation_ref":"work", "sequence":1, "status":200, "summary":"done", "result":{}
        }}),
        json!({"kind":"conversation_execution_failed", "conversation_ref":"one", "invocation_ref":"work", "target_actor_ref":"helper", "error":"failure"}),
        json!({"kind":"conversation_live_output", "conversation_ref":"one", "output":null}),
        json!({"kind":"upstream", "event":{"kind":"stop_requested", "invocation_ref":"work"}}),
        json!({"kind":"upstream", "event":{"kind":"work_requested", "work":{
            "invocation_ref":"work", "root_invocation_ref":"root", "conversation_ref":"one", "requester_actor_ref":"person",
            "route":{"provider_session_ref":"provider", "coworker":{"actor_ref":"tui", "coworker_ref":"tui", "handle":"@tui", "name":"TUI", "actions":[]}},
            "action_name":"show--thing", "input":{}
        }}}),
        json!({"kind":"runtime_restarting", "generation":"replacement"}),
    ];
    for barrier in barriers {
        let barrier: RuntimeServiceEvent = serde_json::from_value(barrier).unwrap();
        let queue = OutputQueue::default();
        push(&queue, &upsert("one", "before"));
        push(&queue, &barrier);
        push(&queue, &upsert("one", "obsolete after"));
        push(&queue, &upsert("one", "latest after"));
        assert_eq!(
            drain(&queue),
            vec![
                upsert("one", "before"),
                barrier,
                upsert("one", "latest after")
            ]
        );
    }
}

#[test]
fn conversation_and_turn_status_snapshots_coalesce_independently() {
    let queue = OutputQueue::default();
    let status = |sequence| {
        serde_json::from_value(json!({
            "kind":"conversation_turn_status_snapshot",
            "conversation_ref":"one",
            "turns":[{
                "invocation_ref":"work",
                "source_message_ref":"message",
                "requester_actor_ref":"person",
                "target_actor_ref":"helper",
                "state":"running",
                "admitted_at_unix_ms":sequence,
                "workers":[]
            }]
        }))
        .unwrap()
    };
    push(&queue, &upsert("one", "old"));
    push(&queue, &status(1));
    push(&queue, &upsert("one", "new"));
    push(&queue, &status(2));
    assert_eq!(drain(&queue), vec![upsert("one", "new"), status(2)]);
}

#[test]
fn snapshot_batches_remain_atomic_and_close_drains_without_a_queue_slot() {
    let queue = OutputQueue::default();
    let events = vec![
        upsert("one", "snapshot"),
        RuntimeServiceEvent::CommandFailed {
            request_id: "snapshot-result".into(),
            error: "result".into(),
        },
    ];
    queue.push(encode_events(&events).unwrap()).unwrap();
    push(&queue, &upsert("one", "later"));
    for _ in 2..OUTPUT_QUEUE_CAPACITY - 1 {
        push(&queue, &events[1]);
    }
    queue.close();
    assert!(matches!(
        queue.push(encode_events(&events).unwrap()),
        Err(EnqueueFailure::Closing)
    ));
    let received = drain(&queue);
    assert_eq!(&received[..2], &events);
    assert_eq!(received[2], upsert("one", "later"));
    assert_eq!(received.len(), OUTPUT_QUEUE_CAPACITY);
}

#[test]
fn full_writer_queue_flushes_restart_notice_before_eof() {
    use std::io::{BufRead, BufReader};
    use std::time::Instant;

    let (client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let writer = client_writer(server).unwrap();
    let first = RuntimeServiceEvent::CommandFailed {
        request_id: "blocked-write".into(),
        error: "x".repeat(2 * 1024 * 1024),
    };
    write_event(&writer, &first).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !writer.queue.pending.lock().unwrap().batches.is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(writer.queue.pending.lock().unwrap().batches.is_empty());
    let mut expected = vec![first];
    for index in 0..OUTPUT_QUEUE_CAPACITY - 1 {
        let event = RuntimeServiceEvent::CommandFailed {
            request_id: index.to_string(),
            error: "queued".into(),
        };
        write_event(&writer, &event).unwrap();
        expected.push(event);
    }
    let restart = vec![
        RuntimeServiceEvent::DrainStatus {
            request_id: "finish".into(),
            remaining: 0,
        },
        RuntimeServiceEvent::RuntimeRestarting {
            generation: "replacement".into(),
        },
    ];
    try_write_restart_events(&writer, &restart).unwrap();
    expected.extend(restart);
    assert_eq!(
        writer.queue.pending.lock().unwrap().batches.len(),
        OUTPUT_QUEUE_CAPACITY
    );
    close_after_output(&writer, crate::idle::Activity::default().delivery());
    assert!(write_event(
        &writer,
        &RuntimeServiceEvent::CommandFailed {
            request_id: "late-reply".into(),
            error: "too late".into(),
        }
    )
    .is_err());
    let received: Vec<RuntimeServiceEvent> = BufReader::new(client)
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect();
    assert_eq!(received, expected);
    assert!(writer.state.failure.get().is_none());
    assert!(!writer_is_connected(&writer));
}

#[test]
fn near_limit_batch_uses_the_single_oversized_reservation() {
    let queue = OutputQueue::default();
    let batch = QueuedFrames::new(
        vec![vec![0; OUTPUT_QUEUE_NON_RESTART_BYTE_CAPACITY + 1]],
        None,
    )
    .unwrap();

    queue.push(batch).expect("admit one near-limit batch");
    let pending = queue.pending.lock().unwrap();
    assert_eq!(pending.budgeted_bytes, 0);
    assert_eq!(pending.oversized, 1);
    drop(pending);

    let batch = queue.next().unwrap();
    queue.complete(&batch);
    assert_eq!(queue.pending.lock().unwrap().oversized, 0);
}

#[test]
fn oversized_replacement_and_failed_admission_preserve_reservations() {
    let queue = OutputQueue::default();
    push(
        &queue,
        &upsert("one", &"x".repeat(OUTPUT_QUEUE_BYTE_CAPACITY + 1)),
    );
    let latest = upsert("one", &"y".repeat(OUTPUT_QUEUE_BYTE_CAPACITY + 2));
    push(&queue, &latest);
    assert!(matches!(
        queue.push(
            encode_events(&[upsert("two", &"z".repeat(OUTPUT_QUEUE_BYTE_CAPACITY + 1))]).unwrap()
        ),
        Err(EnqueueFailure::ByteBudget)
    ));
    assert_eq!(queue.pending.lock().unwrap().oversized, 1);
    assert_eq!(drain(&queue), vec![latest]);
    assert_eq!(queue.pending.lock().unwrap().oversized, 0);
}

#[test]
fn waiting_read_output_observes_cancellation_and_retirement() {
    use std::sync::mpsc;
    use std::time::Instant;
    for cancel in [false, true] {
        let queue = Arc::new(OutputQueue::default());
        for _ in 0..OUTPUT_QUEUE_CAPACITY - 1 {
            queue
                .push(QueuedFrames::new(vec![b"{}\n".to_vec()], None).unwrap())
                .unwrap();
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let waiting_queue = Arc::clone(&queue);
        let waiting_cancelled = Arc::clone(&cancelled);
        let (finished, completion) = mpsc::channel();
        let waiting = thread::spawn(move || {
            let error = waiting_queue
                .push_read(
                    QueuedFrames::new(vec![b"{}\n".to_vec()], None).unwrap(),
                    Instant::now() + Duration::from_secs(3),
                    || waiting_cancelled.load(Ordering::Acquire),
                )
                .unwrap_err();
            finished.send(error.kind()).unwrap();
        });
        assert!(completion.recv_timeout(Duration::from_millis(50)).is_err());
        if cancel {
            cancelled.store(true, Ordering::Release);
        } else {
            queue.retire();
        }
        assert_eq!(
            completion.recv_timeout(Duration::from_secs(1)).unwrap(),
            if cancel {
                io::ErrorKind::Interrupted
            } else {
                io::ErrorKind::BrokenPipe
            }
        );
        waiting.join().unwrap();
    }
}
