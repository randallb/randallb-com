#[cfg(test)]
mod process_stop_precedence_tests {
    use super::*;

    fn completed_commentary(model_run_ref: &str) -> AdapterActivityEvent {
        AdapterActivityEvent::CompletedCommentary {
            commentary: crate::adapter::AdapterCompletedCommentary {
                model_run_ref: model_run_ref.into(),
                provider: "codex".into(),
                thread_id: Some("thread-1".into()),
                response_or_turn_id: "turn-1".into(),
                item_id: "item-1".into(),
                text: "Still working".into(),
            },
        }
    }

    #[test]
    fn completed_commentary_must_match_the_claimed_model_run() {
        assert!(validate_activity_event(
            Some("cwp://com.example.runtime/model_runs/current"),
            &completed_commentary("cwp://com.example.runtime/model_runs/current"),
        )
        .is_ok());
        assert!(validate_activity_event(
            Some("cwp://com.example.runtime/model_runs/current"),
            &completed_commentary("cwp://com.example.runtime/model_runs/old"),
        )
        .is_err());
        assert!(validate_activity_event(
            None,
            &completed_commentary("cwp://com.example.runtime/model_runs/current")
        )
        .is_err());
    }

    #[test]
    fn queued_protocol_reader_error_yields_to_a_stop_request() {
        let stop = ProcessAdapterStopControl::new();
        let mailbox = ProtocolMailbox::new();
        mailbox.send(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "queued reader failure",
            )));
        stop.stop();

        let received = protocol_line_unless_stopped(
            &stop,
            mailbox
                .try_recv()
                .expect("receive the queued reader error"),
        )
        .expect("a concurrent stop takes precedence over the queued reader error");

        assert!(received.is_none());
    }

    #[test]
    fn clean_process_exit_waits_for_its_buffered_terminal_event() {
        let mailbox = Arc::new(ProtocolMailbox::new());
        let reader_mailbox = Arc::clone(&mailbox);
        let delayed_reader = thread::spawn(move || {
            thread::sleep(ADAPTER_POLL_INTERVAL * 2);
            reader_mailbox.send(Ok(ProtocolDelivery::Event {
                line: "terminal event".into(),
                skipped_progress: None,
                progress_sequence: None,
            }));
        });

        let ProtocolAfterExit::Received(received) =
            receive_protocol_after_exit(&mailbox, &ProcessAdapterStopControl::new())
        else {
            panic!("wait for protocol output after process exit")
        };
        let received = received.expect("the protocol reader succeeded");
        delayed_reader.join().expect("join delayed reader");

        assert!(matches!(received, ProtocolDelivery::Event { line, .. } if line == "terminal event"));
    }
}
