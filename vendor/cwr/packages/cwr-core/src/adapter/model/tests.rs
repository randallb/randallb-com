#[cfg(test)]
mod model_output_mailbox_tests {
    use super::*;
    use crate::open_responses::v2026_04_24::AssistantPhase;
    use serde_json::json;

    const INVOCATION_REF: &str = "cwp://com.example.runtime/invocations/one";
    const MODEL_RUN_REF: &str = "cwp://com.example.runtime/model_runs/one";

    include!("overflow_tests.rs");

    #[test]
    fn adapter_result_presence_preserves_null_without_weakening_field_rules() {
        let completed: AdapterTerminal = serde_json::from_value(json!({
            "invocation_ref": INVOCATION_REF,
            "sequence": 1,
            "status": 204,
            "summary": "No content",
            "result": null
        }))
        .unwrap();
        assert_eq!(completed.result, Some(Value::Null));
        validate_terminal(&completed, &json!({"type": "null"})).unwrap();
        assert!(validate_terminal(&completed, &json!({"type": "object"})).is_err());

        let blocked: AdapterTerminal = serde_json::from_value(json!({
            "invocation_ref": INVOCATION_REF,
            "sequence": 1,
            "status": 451,
            "summary": "Unavailable",
            "error": {"code": "unavailable", "details": null}
        }))
        .unwrap();
        assert_eq!(blocked.error.as_ref().unwrap().details, Some(Value::Null));
        assert_eq!(
            serde_json::to_value(&blocked).unwrap()["error"]["details"],
            Value::Null
        );
        validate_terminal(&blocked, &json!({})).unwrap();

        for invalid in [
            json!({
                "invocation_ref": INVOCATION_REF, "sequence": 1, "status": 204,
                "summary": "Done", "result": null, "error": null
            }),
            json!({
                "invocation_ref": INVOCATION_REF, "sequence": 1, "status": 451,
                "summary": "Blocked", "error": null
            }),
            json!({
                "invocation_ref": INVOCATION_REF, "sequence": 1, "status": 451,
                "summary": "Blocked", "error": {"code": "blocked", "message": null}
            }),
        ] {
            assert!(serde_json::from_value::<AdapterTerminal>(invalid).is_err());
        }

        let forbidden_null: AdapterTerminal = serde_json::from_value(json!({
            "invocation_ref": INVOCATION_REF,
            "sequence": 1,
            "status": 451,
            "summary": "Blocked",
            "result": null,
            "error": {"code": "blocked"}
        }))
        .unwrap();
        assert!(validate_terminal(&forbidden_null, &json!({})).is_err());
    }

    #[test]
    fn action_value_limit_counts_exact_serialized_json_bytes() {
        let exact_plain = Value::String("x".repeat(MAX_ACTION_VALUE_BYTES - 2));
        assert_eq!(serde_json::to_vec(&exact_plain).unwrap().len(), MAX_ACTION_VALUE_BYTES);
        validate_action_value("plain", &exact_plain).unwrap();

        let oversized_plain = Value::String("x".repeat(MAX_ACTION_VALUE_BYTES - 1));
        assert!(validate_action_value("plain", &oversized_plain).is_err());

        let exact_escaped = Value::String("\n".repeat((MAX_ACTION_VALUE_BYTES - 2) / 2));
        assert_eq!(serde_json::to_vec(&exact_escaped).unwrap().len(), MAX_ACTION_VALUE_BYTES);
        validate_action_value("escaped", &exact_escaped).unwrap();
        let oversized_escaped = Value::String(format!(
            "{}\n",
            exact_escaped.as_str().expect("escaped test string")
        ));
        assert!(validate_action_value("escaped", &oversized_escaped).is_err());
    }

    #[test]
    fn dense_json_is_measured_as_encoded_structure_not_source_content() {
        let dense = Value::Array((0..100_000).map(|index| json!({"i": index})).collect());
        let encoded = serde_json::to_vec(&dense).unwrap();
        assert!(encoded.len() > 1_000_000);
        validate_action_value("dense", &dense).unwrap();
    }

    #[test]
    fn nested_action_call_counts_the_action_input_without_its_routing_envelope() {
        let operation = AdapterHostOperation {
            name: "cwr.invoke_action".into(),
            description: "Invoke an action".into(),
            input_schema: json!({
                "x-cwr-decoder-version": "cwp/1",
                "type": "object",
                "required": ["target", "input"],
                "properties": {"target": {"type": "object"}, "input": {}},
                "additionalProperties": false
            }),
            output_schema: json!({}),
        };
        let operations = BTreeMap::from([(operation.name.clone(), operation.clone())]);
        let exact = Value::String("x".repeat(MAX_ACTION_VALUE_BYTES - 2));
        let call = json!({"target": {"route": "metadata"}, "input": exact});
        assert!(serde_json::to_vec(&call).unwrap().len() > MAX_ACTION_VALUE_BYTES);
        validate_adapter_call(
            INVOCATION_REF,
            INVOCATION_REF,
            "exact",
            "cwr.invoke_action",
            &call,
            &operations,
            &mut BTreeSet::new(),
        )
        .unwrap();

        let oversized = json!({
            "target": {"route": "metadata"},
            "input": "x".repeat(MAX_ACTION_VALUE_BYTES - 1)
        });
        assert!(validate_adapter_call(
            INVOCATION_REF,
            INVOCATION_REF,
            "oversized",
            "cwr.invoke_action",
            &oversized,
            &operations,
            &mut BTreeSet::new(),
        )
        .is_err());

        let response = json!({
            "invocation_ref": INVOCATION_REF,
            "sequence": 1,
            "status": 200,
            "summary": "Finished",
            "result": Value::String("x".repeat(MAX_ACTION_VALUE_BYTES - 2))
        });
        assert!(serde_json::to_vec(&response).unwrap().len() > MAX_ACTION_VALUE_BYTES);
        validate_adapter_call_result_value(&operation, &response).unwrap();

        let oversized_response = json!({
            "invocation_ref": INVOCATION_REF,
            "sequence": 1,
            "status": 200,
            "summary": "Finished",
            "result": Value::String("x".repeat(MAX_ACTION_VALUE_BYTES - 1))
        });
        assert!(validate_adapter_call_result_value(&operation, &oversized_response).is_err());

        let failed_response = json!({
            "invocation_ref": INVOCATION_REF,
            "sequence": 1,
            "status": 500,
            "summary": "Failed",
            "error": {
                "code": "provider_failed",
                "details": "x".repeat(MAX_ACTION_VALUE_BYTES)
            }
        });
        assert!(serde_json::to_vec(&failed_response).unwrap().len() > MAX_ACTION_VALUE_BYTES);
        validate_adapter_call_result_value(&operation, &failed_response).unwrap();
    }

    #[test]
    fn terminal_rejects_a_result_one_byte_beyond_the_action_value_limit() {
        let terminal = AdapterTerminal {
            invocation_ref: INVOCATION_REF.into(),
            sequence: 1,
            status: 200,
            summary: "Finished".into(),
            result: Some(Value::String("x".repeat(MAX_ACTION_VALUE_BYTES - 1))),
            error: None,
            continuation: None,
        };
        let error = validate_terminal(&terminal, &json!({})).unwrap_err();
        assert!(error.to_string().contains("16 MiB serialized JSON limit"));
    }

    fn output(event: Value) -> AdapterModelOutput {
        AdapterModelOutput {
            model_run_ref: MODEL_RUN_REF.into(),
            provider: "codex".into(),
            thread_id: Some("thread-1".into()),
            response_or_turn_id: "turn-1".into(),
            phase: AssistantPhase::Commentary,
            event,
        }
    }

    fn added(sequence_number: i64) -> Value {
        json!({
            "type": "response.output_item.added",
            "sequence_number": sequence_number,
            "output_index": 0,
            "item": {
                "type": "message",
                "id": "item-1",
                "status": "in_progress",
                "role": "assistant",
                "content": [],
                "phase": "commentary"
            }
        })
    }

    fn delta(sequence_number: i64, delta: &str) -> Value {
        json!({
            "type": "response.output_text.delta",
            "sequence_number": sequence_number,
            "item_id": "item-1",
            "output_index": 0,
            "content_index": 0,
            "delta": delta
        })
    }

    fn done(sequence_number: i64, text: &str) -> Value {
        json!({
            "type": "response.output_text.done",
            "sequence_number": sequence_number,
            "item_id": "item-1",
            "output_index": 0,
            "content_index": 0,
            "text": text
        })
    }

    fn accept(mailbox: &AdapterModelOutputMailbox, output: AdapterModelOutput) {
        mailbox
            .accept(
                INVOCATION_REF,
                Some(MODEL_RUN_REF),
                INVOCATION_REF.into(),
                output,
            )
            .unwrap();
    }

    #[test]
    fn coalesces_deltas_and_applies_the_exact_done_correction() {
        let mailbox = AdapterModelOutputMailbox::default();
        accept(&mailbox, output(added(0)));
        accept(&mailbox, output(delta(1, "draft")));
        accept(&mailbox, output(delta(1, " ignored")));
        assert_eq!(
            mailbox
                .snapshot()
                .output
                .as_ref()
                .map(|commentary| commentary.text.as_str()),
            Some("draft")
        );

        accept(&mailbox, output(delta(2, " text")));
        accept(&mailbox, output(done(3, "corrected text")));
        let snapshot = mailbox.snapshot();
        let commentary = snapshot.output.unwrap();
        assert_eq!(commentary.text, "corrected text");
        assert_eq!(commentary.sequence_number, 3);
        assert!(snapshot.pending_bytes <= MODEL_OUTPUT_MAILBOX_BYTES);
    }

    #[test]
    fn final_answer_remains_visible_until_the_run_closes() {
        let mailbox = AdapterModelOutputMailbox::default();
        let mut final_added = added(0);
        final_added["item"]["phase"] = json!("final_answer");
        let mut final_output = output(final_added);
        final_output.phase = AssistantPhase::FinalAnswer;
        accept(&mailbox, final_output);
        let text = format!("{}final reply", "🦀".repeat(MODEL_OUTPUT_MAILBOX_BYTES));
        let mut final_delta = output(delta(1, &text));
        final_delta.phase = AssistantPhase::FinalAnswer;
        accept(&mailbox, final_delta);
        let mut final_done = output(json!({
            "type": "response.output_item.done",
            "sequence_number": 2,
            "output_index": 0,
            "item": {
                "type": "message",
                "id": "item-1",
                "status": "completed",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": text,
                    "annotations": []
                }],
                "phase": "final_answer"
            }
        }));
        final_done.phase = AssistantPhase::FinalAnswer;
        accept(&mailbox, final_done);

        let snapshot = mailbox.snapshot();
        let visible = snapshot.output.expect("completed provisional result");
        assert_eq!(visible.phase, AssistantPhase::FinalAnswer);
        assert!(visible.text.starts_with(MODEL_OUTPUT_CLIPPED));
        assert!(visible.text.ends_with("final reply"));
        assert!(snapshot.pending_bytes <= MODEL_OUTPUT_MAILBOX_BYTES);

        mailbox.close_run(INVOCATION_REF, Some(MODEL_RUN_REF));
        accept(&mailbox, output(delta(3, "late output")));
        assert!(mailbox.snapshot().output.is_none());
    }

    #[test]
    fn completed_commentary_and_run_closure_clear_temporary_text() {
        let mailbox = AdapterModelOutputMailbox::default();
        accept(&mailbox, output(added(0)));
        accept(&mailbox, output(delta(1, "unfinished")));
        mailbox.complete_commentary(INVOCATION_REF, &AdapterCompletedCommentary {
            model_run_ref: MODEL_RUN_REF.into(),
            provider: "codex".into(),
            thread_id: Some("thread-1".into()),
            response_or_turn_id: "turn-1".into(),
            item_id: "item-1".into(),
            text: "finished".into(),
        });
        let completed = mailbox.snapshot();
        assert!(completed.output.is_none());

        mailbox.close_run(INVOCATION_REF, Some(MODEL_RUN_REF));
        accept(&mailbox, output(delta(2, "late")));
        let closed = mailbox.snapshot();
        assert!(closed.output.is_none());
        assert!(closed.revision > completed.revision);
        assert_eq!(
            closed.closed_model_runs,
            vec![AdapterClosedModelRun {
                invocation_ref: INVOCATION_REF.into(),
                model_run_ref: MODEL_RUN_REF.into(),
            }]
        );
    }

    #[test]
    fn metadata_that_cannot_fit_still_disables_delivery_safely() {
        let mailbox = AdapterModelOutputMailbox::with_byte_limit(64);
        accept(&mailbox, output(added(0)));
        accept(&mailbox, output(delta(1, &"x".repeat(4096))));
        let snapshot = mailbox.snapshot();
        assert!(snapshot.output.is_none());
        assert_eq!(snapshot.pending_bytes, 0);
    }

    #[test]
    fn lifecycle_bookkeeping_is_included_in_the_byte_limit() {
        let mailbox = AdapterModelOutputMailbox::with_byte_limit(1024);
        for sequence_number in 0..100 {
            accept(
                &mailbox,
                output(json!({
                    "type": "response.output_item.added",
                    "sequence_number": sequence_number,
                    "output_index": sequence_number,
                    "item": {
                        "type": "message",
                        "id": format!("item-{sequence_number}"),
                        "status": "in_progress",
                        "role": "assistant",
                        "content": [],
                        "phase": "commentary"
                    }
                })),
            );
        }
        let snapshot = mailbox.snapshot();
        assert!(snapshot.pending_bytes <= 1024);
        assert!(snapshot.output.is_none());
        assert!(mailbox
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .disabled);
    }

    #[test]
    fn model_error_closes_the_run_and_late_output_cannot_revive_it() {
        let mailbox = AdapterModelOutputMailbox::default();
        accept(&mailbox, output(added(0)));
        accept(&mailbox, output(delta(1, &"unfinished 🦀".repeat(10_000))));
        assert!(mailbox.snapshot().output.unwrap().text.starts_with(MODEL_OUTPUT_CLIPPED));
        let mut failure = output(json!({
                "type": "error",
                "sequence_number": 0,
                "error": {
                    "type": "model_error",
                    "code": "overloaded",
                    "message": "Try again later.",
                    "param": null
                }
            }));
        failure.phase = AssistantPhase::FinalAnswer;
        accept(&mailbox, failure);
        assert!(mailbox.snapshot().output.is_none());

        accept(&mailbox, output(delta(3, "late")));
        assert!(mailbox.snapshot().output.is_none());
    }

    #[test]
    fn durable_completion_tombstones_an_item_without_prior_temporary_output() {
        let mailbox = AdapterModelOutputMailbox::default();
        mailbox.complete_commentary(
            INVOCATION_REF,
            &AdapterCompletedCommentary {
                model_run_ref: MODEL_RUN_REF.into(),
                provider: "codex".into(),
                thread_id: Some("thread-1".into()),
                response_or_turn_id: "turn-1".into(),
                item_id: "item-1".into(),
                text: "finished".into(),
            },
        );

        accept(&mailbox, output(added(0)));
        accept(&mailbox, output(delta(1, "late")));
        assert!(mailbox.snapshot().output.is_none());
    }

    #[test]
    fn stop_closes_the_registered_run_before_late_output() {
        let mailbox = AdapterModelOutputMailbox::default();
        let stop = ProcessAdapterStopControl::new();
        stop.register_model_output_run(
            mailbox.clone(),
            INVOCATION_REF.into(),
            Some(MODEL_RUN_REF.into()),
        );
        accept(&mailbox, output(added(0)));
        accept(&mailbox, output(delta(1, &"unfinished 🦀".repeat(10_000))));
        assert!(mailbox.snapshot().output.unwrap().text.starts_with(MODEL_OUTPUT_CLIPPED));

        stop.stop();
        accept(&mailbox, output(delta(2, "late")));
        assert!(mailbox.snapshot().output.is_none());
    }

    #[test]
    fn protocol_reader_applies_result_closure_before_later_model_output() {
        let mailbox = AdapterModelOutputMailbox::default();
        accept(&mailbox, output(added(0)));
        accept(&mailbox, output(delta(1, "unfinished")));
        let protocol = ProtocolMailbox::new();
        assert!(!send_protocol_line(
            &protocol,
            br#"{"type":"result"}"#,
            INVOCATION_REF,
            Some(MODEL_RUN_REF),
            &mailbox,
            &AdapterTimingSink::default(),
        ));
        assert!(protocol.try_recv().is_ok());

        let late = serde_json::to_vec(&AdapterEvent::ModelOutput {
            invocation_ref: INVOCATION_REF.into(),
            output: output(delta(2, "late")),
        })
        .unwrap();
        assert!(send_protocol_line(
            &protocol,
            &late,
            INVOCATION_REF,
            Some(MODEL_RUN_REF),
            &mailbox,
            &AdapterTimingSink::default(),
        ));
        assert!(mailbox.snapshot().output.is_none());
        assert!(protocol.try_recv().is_err());
    }

    #[test]
    fn protocol_reader_applies_completed_item_closure_before_later_output() {
        let mailbox = AdapterModelOutputMailbox::default();
        accept(&mailbox, output(added(0)));
        accept(&mailbox, output(delta(1, "unfinished")));
        let activity = serde_json::to_vec(&AdapterEvent::Activity {
            invocation_ref: INVOCATION_REF.into(),
            sequence: 1,
            event: AdapterActivityEvent::CompletedCommentary {
                commentary: AdapterCompletedCommentary {
                    model_run_ref: MODEL_RUN_REF.into(),
                    provider: "codex".into(),
                    thread_id: Some("thread-1".into()),
                    response_or_turn_id: "turn-1".into(),
                    item_id: "item-1".into(),
                    text: "finished".into(),
                },
            },
        })
        .unwrap();
        let protocol = ProtocolMailbox::new();
        assert!(send_protocol_line(
            &protocol,
            &activity,
            INVOCATION_REF,
            Some(MODEL_RUN_REF),
            &mailbox,
            &AdapterTimingSink::default(),
        ));
        assert!(protocol.try_recv().is_ok());

        let late = serde_json::to_vec(&AdapterEvent::ModelOutput {
            invocation_ref: INVOCATION_REF.into(),
            output: output(delta(2, "late")),
        })
        .unwrap();
        assert!(send_protocol_line(
            &protocol,
            &late,
            INVOCATION_REF,
            Some(MODEL_RUN_REF),
            &mailbox,
            &AdapterTimingSink::default(),
        ));
        assert!(mailbox.snapshot().output.is_none());
    }
}
