#[cfg(test)]
mod tests {
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::*;

    fn message(id: &str, status: &str, phase: Option<&str>, text: Option<&str>) -> Value {
        let mut message = json!({
            "type": "message",
            "id": id,
            "status": status,
            "role": "assistant",
            "content": text.map(|text| vec![json!({
                "type": "output_text",
                "text": text,
                "annotations": []
            })]).unwrap_or_default()
        });
        if let Some(phase) = phase {
            message["phase"] = json!(phase);
        }
        message
    }

    fn function_call(id: &str, status: &str, call_id: &str, name: &str, arguments: &str) -> Value {
        json!({
            "type": "function_call",
            "id": id,
            "call_id": call_id,
            "name": name,
            "arguments": arguments,
            "status": status
        })
    }

    fn item_event(kind: &str, sequence: i64, output_index: i64, item: Value) -> Value {
        json!({
            "type": kind,
            "sequence_number": sequence,
            "output_index": output_index,
            "item": item
        })
    }

    fn part_event(
        kind: &str,
        sequence: i64,
        item_id: &str,
        output_index: i64,
        content_index: i64,
        text: &str,
    ) -> Value {
        json!({
            "type": kind,
            "sequence_number": sequence,
            "item_id": item_id,
            "output_index": output_index,
            "content_index": content_index,
            "part": {"type": "output_text", "text": text, "annotations": []}
        })
    }

    fn delta_event(
        sequence: i64,
        item_id: &str,
        output_index: i64,
        content_index: i64,
        delta: &str,
    ) -> Value {
        json!({
            "type": "response.output_text.delta",
            "sequence_number": sequence,
            "item_id": item_id,
            "output_index": output_index,
            "content_index": content_index,
            "delta": delta
        })
    }

    fn text_done_event(
        sequence: i64,
        item_id: &str,
        output_index: i64,
        content_index: i64,
        text: &str,
    ) -> Value {
        json!({
            "type": "response.output_text.done",
            "sequence_number": sequence,
            "item_id": item_id,
            "output_index": output_index,
            "content_index": content_index,
            "text": text
        })
    }

    #[test]
    fn checked_in_documents_match_the_pinned_profile() {
        let openapi = upstream_openapi_document().expect("valid pinned OpenAPI");
        assert_eq!(openapi["info"]["version"], PROFILE_VERSION);
        assert_eq!(OPENAPI_JSON.len(), UPSTREAM_OPENAPI_BYTES);
        assert_eq!(
            hex::encode(Sha256::digest(OPENAPI_JSON.as_bytes())),
            UPSTREAM_OPENAPI_SHA256
        );

        let profile = profile_document().expect("valid checked-in profile");
        assert_eq!(profile["profile"], PROFILE_NAME);
        assert_eq!(profile["profile_version"], PROFILE_VERSION);
        assert_eq!(profile["upstream"]["sha256"], UPSTREAM_OPENAPI_SHA256);
    }

    #[test]
    fn request_projection_accepts_text_items_and_continuation() {
        let request = json!({
            "model": "provider-model",
            "previous_response_id": "resp_previous",
            "input": [
                {"type": "message", "role": "developer", "content": "Be concise."},
                {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "Hello"}
                ]},
                {"type": "function_call_output", "call_id": "call_1", "output": "42"}
            ]
        });
        let request = validate_request(&request).expect("supported request");
        assert_eq!(
            request.previous_response_id.as_deref(),
            Some("resp_previous")
        );
        assert_eq!(request.input_item_count, 3);
        assert!(request.has_text_input);

        let empty_continuation = validate_request(&json!({"previous_response_id": ""}))
            .expect("pinned schema permits an empty continuation string");
        assert_eq!(empty_continuation.previous_response_id.as_deref(), Some(""));
    }

    #[test]
    fn request_projection_rejects_transport_and_non_text_input() {
        let transport = json!({"input": "hello", "stream": true});
        assert!(validate_request(&transport)
            .expect_err("transport field is outside profile")
            .to_string()
            .contains("unsupported field stream"));

        let image = json!({
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_image", "image_url": "https://example.com/a.png"}]
            }]
        });
        assert!(validate_request(&image).is_err());
    }

    #[test]
    fn input_items_require_exact_standard_fields() {
        let function_call = json!({
            "type": "function_call",
            "call_id": "call_1",
            "name": "lookup",
            "arguments": "{}"
        });
        assert_eq!(
            validate_input_item(&function_call).unwrap().kind,
            InputItemKind::FunctionCall
        );

        let missing_call_id = json!({"type": "function_call", "name": "lookup", "arguments": "{}"});
        assert!(validate_input_item(&missing_call_id).is_err());
        let extension = json!({
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "ok",
            "provider:trace": "private"
        });
        assert!(validate_input_item(&extension).is_err());
    }

    #[test]
    fn assistant_phase_is_required_by_profile_and_unknown_phase_fails_upstream() {
        let no_phase = message("msg_1", "in_progress", None, None);
        assert!(validate_output_item(&no_phase).is_err());

        let commentary = message("msg_2", "in_progress", Some("commentary"), None);
        assert_eq!(
            validate_output_item(&commentary).unwrap().phase,
            Some(AssistantPhase::Commentary)
        );

        let unknown = message("msg_3", "in_progress", Some("analysis"), None);
        assert!(validate_output_item(&unknown).is_err());
    }

    #[test]
    fn output_union_rejects_non_assistant_and_non_text_items() {
        let mut user = message("msg_1", "completed", Some("commentary"), Some("hello"));
        user["role"] = json!("user");
        assert!(validate_output_item(&user).is_err());

        let reasoning = json!({
            "type": "reasoning",
            "id": "reasoning_1",
            "status": "completed",
            "summary": []
        });
        assert!(validate_output_item(&reasoning).is_err());
    }

    #[test]
    fn output_string_fields_follow_the_pinned_schema_without_hidden_length_rules() {
        assert_eq!(
            validate_output_item(&message("", "in_progress", Some("commentary"), None))
                .expect("empty message ID is a schema-valid string")
                .item_id,
            ""
        );
        let call = validate_output_item(&function_call("", "in_progress", "", "", ""))
            .expect("function-call strings follow the pinned schema");
        assert_eq!(call.item_id, "");
        assert_eq!(call.call_id.as_deref(), Some(""));
        assert_eq!(call.function_name.as_deref(), Some(""));
    }

    #[test]
    fn function_call_lifecycle_preserves_identity_name_and_arguments() {
        let mut stream = StreamValidator::new();
        let added = stream
            .accept(&item_event(
                "response.output_item.added",
                1,
                0,
                function_call("fc_1", "in_progress", "call_1", "lookup", "{\"id\":1}"),
            ))
            .expect("function-call start");
        assert_eq!(added.call_id.as_deref(), Some("call_1"));
        assert_eq!(added.function_name.as_deref(), Some("lookup"));
        assert_eq!(added.arguments.as_deref(), Some("{\"id\":1}"));

        for changed in [
            function_call("fc_1", "completed", "call_2", "lookup", "{\"id\":1}"),
            function_call("fc_1", "completed", "call_1", "other", "{\"id\":1}"),
            function_call("fc_1", "completed", "call_1", "lookup", "{\"id\":2}"),
        ] {
            assert!(stream
                .accept(&item_event("response.output_item.done", 2, 0, changed,))
                .is_err());
        }

        stream
            .accept(&item_event(
                "response.output_item.done",
                2,
                0,
                function_call("fc_1", "completed", "call_1", "lookup", "{\"id\":1}"),
            ))
            .expect("matching function-call completion");
    }

    #[test]
    fn supported_event_union_is_explicit() {
        let event = item_event(
            "response.output_item.added",
            1,
            0,
            message("msg_1", "in_progress", Some("commentary"), None),
        );
        assert_eq!(
            validate_event(&event).unwrap().kind,
            EventKind::OutputItemAdded
        );

        for unsupported in ["response.completed", "response.created", "openai:trace"] {
            assert!(validate_event(&json!({"type": unsupported, "sequence_number": 2})).is_err());
        }
    }

    #[test]
    fn stream_correlates_interleaved_assistant_phases() {
        let mut stream = StreamValidator::new();
        stream
            .accept(&item_event(
                "response.output_item.added",
                1,
                0,
                message("commentary", "in_progress", Some("commentary"), None),
            ))
            .unwrap();
        stream
            .accept(&item_event(
                "response.output_item.added",
                2,
                1,
                message("answer", "in_progress", Some("final_answer"), None),
            ))
            .unwrap();
        stream
            .accept(&part_event(
                "response.content_part.added",
                3,
                "answer",
                1,
                0,
                "",
            ))
            .unwrap();
        stream
            .accept(&part_event(
                "response.content_part.added",
                4,
                "commentary",
                0,
                0,
                "",
            ))
            .unwrap();

        let answer = stream
            .accept(&delta_event(5, "answer", 1, 0, "Final"))
            .unwrap();
        assert_eq!(answer.phase, Some(AssistantPhase::FinalAnswer));
        let commentary = stream
            .accept(&delta_event(6, "commentary", 0, 0, "Working"))
            .unwrap();
        assert_eq!(commentary.phase, Some(AssistantPhase::Commentary));
    }

    #[test]
    fn full_snapshots_repair_deltas_and_completed_items() {
        let mut stream = StreamValidator::new();
        stream
            .accept(&item_event(
                "response.output_item.added",
                1,
                0,
                message("msg_1", "in_progress", Some("commentary"), None),
            ))
            .unwrap();
        stream
            .accept(&part_event(
                "response.content_part.added",
                2,
                "msg_1",
                0,
                0,
                "",
            ))
            .unwrap();
        assert_eq!(
            stream
                .accept(&delta_event(3, "msg_1", 0, 0, "drfat"))
                .unwrap()
                .accumulated_text
                .as_deref(),
            Some("drfat")
        );
        assert_eq!(
            stream
                .accept(&text_done_event(4, "msg_1", 0, 0, "draft"))
                .unwrap()
                .accumulated_text
                .as_deref(),
            Some("draft")
        );
        stream
            .accept(&part_event(
                "response.content_part.done",
                5,
                "msg_1",
                0,
                0,
                "draft",
            ))
            .unwrap();
        let done = stream
            .accept(&item_event(
                "response.output_item.done",
                6,
                0,
                message("msg_1", "completed", Some("commentary"), Some("draft")),
            ))
            .unwrap();
        assert_eq!(done.accumulated_text.as_deref(), Some("draft"));
        assert_eq!(done.phase, Some(AssistantPhase::Commentary));
    }

    #[test]
    fn stream_rejects_invalid_order_and_stable_identity_violations() {
        let mut stream = StreamValidator::new();
        stream
            .accept(&item_event(
                "response.output_item.added",
                1,
                0,
                message("msg_1", "in_progress", Some("commentary"), None),
            ))
            .unwrap();
        assert!(stream
            .accept(&delta_event(2, "msg_1", 0, 0, "early"))
            .is_err());
        stream
            .accept(&part_event(
                "response.content_part.added",
                2,
                "msg_1",
                0,
                0,
                "",
            ))
            .unwrap();
        assert!(stream
            .accept(&delta_event(2, "msg_1", 0, 0, "duplicate"))
            .is_err());
        assert!(stream
            .accept(&delta_event(3, "msg_1", 1, 0, "wrong index"))
            .is_err());
    }

    #[test]
    fn closed_items_cannot_receive_late_output_or_reopen() {
        let mut stream = StreamValidator::new();
        stream
            .accept(&item_event(
                "response.output_item.added",
                1,
                0,
                message("msg_1", "in_progress", Some("commentary"), None),
            ))
            .unwrap();
        stream
            .accept(&item_event(
                "response.output_item.done",
                2,
                0,
                message("msg_1", "completed", Some("commentary"), None),
            ))
            .unwrap();
        assert!(stream
            .accept(&part_event(
                "response.content_part.added",
                3,
                "msg_1",
                0,
                0,
                "late",
            ))
            .is_err());
        assert!(stream
            .accept(&item_event(
                "response.output_item.added",
                3,
                0,
                message("msg_1", "in_progress", Some("commentary"), None),
            ))
            .is_err());
    }

    #[test]
    fn structured_error_closes_the_supported_stream() {
        let mut stream = StreamValidator::new();
        let error = json!({
            "type": "error",
            "sequence_number": 1,
            "error": {
                "type": "model_error",
                "code": "overloaded",
                "message": "Try again later.",
                "param": null
            }
        });
        let accepted = stream.accept(&error).expect("structured error");
        assert_eq!(accepted.error.unwrap().code.as_deref(), Some("overloaded"));
        assert!(stream.is_closed());
        assert!(stream
            .accept(&item_event(
                "response.output_item.added",
                2,
                0,
                message("late", "in_progress", Some("commentary"), None),
            ))
            .is_err());
    }
}
