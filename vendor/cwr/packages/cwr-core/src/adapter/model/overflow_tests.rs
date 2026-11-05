#[test]
fn overflowing_text_keeps_a_live_unicode_tail_and_accepts_corrections() {
    let mailbox = AdapterModelOutputMailbox::default();
    accept(&mailbox, output(added(0)));
    let chunk = "🦀 café\n\"quoted\"\\\u{0000}".repeat(200);
    let mut full = String::new();
    let mut revision = 0;
    for sequence in 1..=80 {
        full.push_str(&chunk);
        accept(&mailbox, output(delta(sequence, &chunk)));
        let snapshot = mailbox.snapshot();
        let visible = snapshot.output.as_ref().unwrap();
        assert_eq!(visible.sequence_number, sequence);
        assert!(snapshot.revision > revision);
        assert!(snapshot.pending_bytes <= MODEL_OUTPUT_MAILBOX_BYTES);
        let tail = visible
            .text
            .strip_prefix(MODEL_OUTPUT_CLIPPED)
            .unwrap_or(&visible.text);
        assert!(full.ends_with(tail));
        assert!(tail.ends_with(&chunk));
        revision = snapshot.revision;
    }
    let clipped = mailbox.snapshot();
    assert!(clipped
        .output
        .as_ref()
        .unwrap()
        .text
        .starts_with(MODEL_OUTPUT_CLIPPED));
    accept(&mailbox, output(delta(80, "duplicate")));
    accept(&mailbox, output(delta(79, "out of order")));
    assert_eq!(mailbox.snapshot(), clipped);

    let correction = format!("{}corrected ending", "新しい\n".repeat(20_000));
    accept(&mailbox, output(done(81, &correction)));
    let corrected = mailbox.snapshot();
    let tail = corrected
        .output
        .as_ref()
        .unwrap()
        .text
        .strip_prefix(MODEL_OUTPUT_CLIPPED)
        .unwrap();
    assert!(correction.ends_with(tail));
    assert!(tail.ends_with("corrected ending"));
    assert!(corrected.pending_bytes <= MODEL_OUTPUT_MAILBOX_BYTES);

    accept(&mailbox, output(done(82, "short exact correction")));
    assert_eq!(
        mailbox.snapshot().output.unwrap().text,
        "short exact correction"
    );
}

#[test]
fn clipping_obeys_json_escape_costs_at_every_character_boundary() {
    let text: String = (0..=127)
        .filter_map(char::from_u32)
        .chain("é中🦀".chars())
        .collect();
    for limit in 0..=1024 {
        let tail = model_output_tail(&text, limit);
        let cost = serde_json::to_vec(tail).unwrap().len() - 2;
        assert!(cost <= limit);
        let prefix = &text[..text.len() - tail.len()];
        if let Some((index, _)) = prefix.char_indices().next_back() {
            assert!(serde_json::to_vec(&text[index..]).unwrap().len() - 2 > limit);
        }
    }
}

#[test]
fn completed_clipped_commentary_cannot_reopen_and_next_item_still_streams() {
    let mailbox = AdapterModelOutputMailbox::with_byte_limit(2048);
    accept(&mailbox, output(added(0)));
    accept(&mailbox, output(delta(1, &"中".repeat(3000))));
    assert!(mailbox
        .snapshot()
        .output
        .unwrap()
        .text
        .starts_with(MODEL_OUTPUT_CLIPPED));
    mailbox.complete_commentary(
        INVOCATION_REF,
        &AdapterCompletedCommentary {
            model_run_ref: MODEL_RUN_REF.into(),
            provider: "codex".into(),
            thread_id: Some("thread-1".into()),
            response_or_turn_id: "turn-1".into(),
            item_id: "item-1".into(),
            text: "durable completion".into(),
        },
    );
    accept(&mailbox, output(added(2)));
    accept(&mailbox, output(delta(3, "late")));
    assert!(mailbox.snapshot().output.is_none());

    let mut next_added = added(4);
    next_added["item"]["id"] = json!("item-2");
    accept(&mailbox, output(next_added));
    let mut next_delta = delta(5, "new item");
    next_delta["item_id"] = json!("item-2");
    accept(&mailbox, output(next_delta));
    assert_eq!(mailbox.snapshot().output.unwrap().text, "new item");
}

#[test]
fn overflow_does_not_clip_the_terminal_result_or_lose_run_closure() {
    let mailbox = AdapterModelOutputMailbox::default();
    accept(&mailbox, output(added(0)));
    let text = format!("{}final diagnostic", "é".repeat(40_000));
    accept(&mailbox, output(delta(1, &text)));
    assert!(mailbox
        .snapshot()
        .output
        .unwrap()
        .text
        .starts_with(MODEL_OUTPUT_CLIPPED));
    let protocol = ProtocolMailbox::new();
    let terminal = json!({
        "type": "result", "invocation_ref": INVOCATION_REF,
        "sequence": 1, "status": 200, "summary": "Done", "result": {"text": text}
    });
    assert!(!send_protocol_line(
        &protocol,
        &serde_json::to_vec(&terminal).unwrap(),
        INVOCATION_REF,
        Some(MODEL_RUN_REF),
        &mailbox,
        &AdapterTimingSink::default(),
    ));
    let received = protocol.try_recv().unwrap().unwrap();
    let ProtocolDelivery::Terminal { line, .. } = received else {
        panic!("result must use terminal delivery")
    };
    let event: AdapterEvent = serde_json::from_str(&line).unwrap();
    assert!(
        matches!(event, AdapterEvent::Result { result: Some(result), .. } if result["text"] == text)
    );
    accept(&mailbox, output(added(2)));
    accept(&mailbox, output(delta(3, "late")));
    let closed = mailbox.snapshot();
    assert!(closed.output.is_none());
    assert_eq!(closed.closed_model_runs.len(), 1);
    assert!(closed.pending_bytes <= MODEL_OUTPUT_MAILBOX_BYTES);
}
