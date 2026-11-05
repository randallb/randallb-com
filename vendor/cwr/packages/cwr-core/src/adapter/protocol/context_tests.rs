use super::*;

struct MeasuringWriter<W> {
    writer: W,
    line_bytes: usize,
    max_line_bytes: usize,
    total_bytes: usize,
}

impl<W: Write> Write for MeasuringWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.writer.write(bytes)?;
        for byte in &bytes[..written] {
            self.line_bytes += 1;
            self.total_bytes += 1;
            if *byte == b'\n' {
                self.max_line_bytes = self.max_line_bytes.max(self.line_bytes);
                self.line_bytes = 0;
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[test]
fn complete_context_crosses_one_four_and_sixteen_mib_with_small_frames() {
    for mebibytes in [1, 4, 16] {
        // One large individual body, including characters split across byte
        // chunks, must not become different source turns or lose its tail.
        let body = "🔎\n\"".repeat(mebibytes * 1024 * 1024 / 6);
        let context = json!({
            "conversation_ref": "cwp://example.test/conversations/root",
            "messages": [
                {"message_ref": "first", "author_actor_ref": "Alice", "text": "Keep the first constraint"},
                {"message_ref": "large", "author_actor_ref": "Bob", "reply_to_message_ref": "first", "text": body},
                {"message_ref": "last", "author_actor_ref": "Alice", "text": "Apply this later correction"}
            ]
        });
        let (sender, receiver) = UnixStream::pair().unwrap();
        let started = Instant::now();
        let writer = thread::spawn(move || {
            let mut writer = MeasuringWriter {
                writer: sender,
                line_bytes: 0,
                max_line_bytes: 0,
                total_bytes: 0,
            };
            let mut invocation = AdapterInvocation::new(
                "cwp://example.test/invocations/one",
                "test",
                json!({}),
                json!({}),
            );
            invocation.thread = context_stream_descriptor();
            write_adapter_message(
                &mut writer,
                &AdapterHostMessage::Invoke {
                    protocol: PROCESS_ADAPTER_PROTOCOL_REF.into(),
                    invocation: Box::new(invocation),
                },
            )
            .unwrap();
            write_context_stream(&mut writer, "cwp://example.test/invocations/one", &context)
                .unwrap();
            (writer.max_line_bytes, writer.total_bytes)
        });
        let message: AdapterHostMessage = read_adapter_message(&mut io::BufReader::new(receiver))
            .unwrap()
            .unwrap();
        let AdapterHostMessage::Invoke { invocation, .. } = message else {
            panic!("invoke")
        };
        assert_eq!(
            invocation.thread["messages"][0]["text"],
            "Keep the first constraint"
        );
        assert_eq!(invocation.thread["messages"][1]["text"], body);
        assert_eq!(
            invocation.thread["messages"][1]["reply_to_message_ref"],
            "first"
        );
        assert_eq!(
            invocation.thread["messages"][2]["text"],
            "Apply this later correction"
        );
        let (max_frame, total) = writer.join().unwrap();
        assert!(max_frame < CONTEXT_CHUNK_BYTES * 2 + 1024);
        eprintln!("context {mebibytes} MiB: wire={total} bytes, max_frame={max_frame} bytes, elapsed={:?}", started.elapsed());
    }
}

#[test]
fn context_stream_rejects_missing_duplicate_and_foreign_chunks() {
    for (invocation_ref, sequence) in [("other", 1), ("expected", 2)] {
        let bytes = encode_adapter_message(&ContextFrame::ContextChunk {
            invocation_ref: invocation_ref.into(),
            sequence,
            data: hex::encode(b"{}"),
        })
        .unwrap();
        let mut input = io::Cursor::new(bytes);
        assert!(
            serde_json::from_reader::<_, Value>(ContextStreamReader::new(
                &mut input,
                "expected".into()
            ))
            .is_err()
        );
    }
    let bytes = encode_adapter_message(&ContextFrame::ContextChunk {
        invocation_ref: "expected".into(),
        sequence: 1,
        data: hex::encode(b"{}"),
    })
    .unwrap();
    let mut input = io::Cursor::new(bytes);
    assert!(
        serde_json::from_reader::<_, Value>(ContextStreamReader::new(
            &mut input,
            "expected".into()
        ))
        .is_err()
    );
}

#[test]
fn oversized_writes_are_rejected_without_partial_output() {
    let mut output = Vec::new();
    let error = write_adapter_message(
        &mut output,
        &json!({"body": "x".repeat(MAX_PROTOCOL_LINE_BYTES)}),
    )
    .expect_err("oversized control frame");
    assert!(error.to_string().contains("exceeds 32 MiB"));
    assert!(output.is_empty());
}

#[test]
fn oversized_line_without_newline_is_rejected_before_reading_the_body() {
    let mut reader = io::BufReader::with_capacity(8192, io::repeat(b'x').take(64 * 1024 * 1024));
    let error = read_protocol_line(&mut reader).expect_err("oversized unterminated line");
    assert!(error.to_string().contains("exceeds 32 MiB"));
    assert!(reader.get_ref().limit() >= 30 * 1024 * 1024);
}

#[test]
fn cancelling_reader_releases_a_full_queue() {
    let (mut producer, stdout) = UnixStream::pair().unwrap();
    let (full, wait_until_full) = mpsc::channel();
    let reader = spawn_protocol_reader(
        stdout,
        "invocation".into(),
        None,
        AdapterModelOutputMailbox::discard(),
        AdapterTimingSink::new(move |_| full.send(()).unwrap()),
    )
    .unwrap();
    let producer_thread = thread::spawn(move || {
        for _ in 0..PROTOCOL_QUEUE_FRAMES {
            producer.write_all(b"{\"type\":\"unknown\"}\n").unwrap();
        }
        // Timing bypasses the queue, so this callback confirms all four prior
        // frames were accepted before we cancel a reader applying backpressure.
        write_adapter_message(
            &mut producer,
            &AdapterEvent::Timing {
                invocation_ref: "invocation".into(),
                model_run_ref: None,
                timing: AdapterTiming::single(
                    TimingCheckpoint::FirstText,
                    TimingDuration::FirstText,
                    1,
                ),
            },
        )
        .unwrap();
        // Once the fifth frame is pending, the producer cannot queue further
        // complete frames. Dropping the receiver must release that blocked send.
        for _ in 0..1000 {
            if producer.write_all(b"{\"type\":\"unknown\"}\n").is_err() {
                break;
            }
        }
    });
    wait_until_full
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    let started = Instant::now();
    let reader_thread = cancel_protocol_reader(reader);
    join_protocol_reader_thread(reader_thread).unwrap();
    producer_thread.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
}
