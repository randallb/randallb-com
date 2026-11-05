mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn turn_filter_consumes_every_matching_activity_in_one_upsert() {
        let receipt = HostCommandReceipt {
            attachment_ref: None,
            conversation_ref: "cwp://example.test/conversations/one".into(),
            message_ref: Some("cwp://example.test/messages/one".into()),
            invocation_ref: Some("cwp://example.test/invocations/one".into()),
            stopped_invocation_refs: Vec::new(),
        };
        let conversation = serde_json::from_value(json!({
            "conversation_ref": receipt.conversation_ref,
            "participant_actor_refs": ["~@/actors/person", "~@/actors/helper"],
            "messages": [],
            "activity": [
                {
                    "activity_ref": "cwp://example.test/activities/one",
                    "sequence": 1,
                    "event": {
                        "kind": "work_update",
                        "invocation_ref": "cwp://example.test/invocations/one",
                        "source_message_ref": "cwp://example.test/messages/one",
                        "actor_ref": "~@/actors/helper",
                        "status": 102,
                        "text": "First"
                    }
                },
                {
                    "activity_ref": "cwp://example.test/activities/two",
                    "sequence": 2,
                    "event": {
                        "kind": "work_update",
                        "invocation_ref": "cwp://example.test/invocations/one",
                        "source_message_ref": "cwp://example.test/messages/one",
                        "actor_ref": "~@/actors/helper",
                        "status": 102,
                        "text": "Second"
                    }
                }
            ]
        }))
        .unwrap();
        let event = HostEvent::ConversationUpsert {
            source_ref: "cwp://example.test/sources/local".into(),
            conversation,
        };
        let mut observed = std::collections::BTreeSet::new();

        assert!(event_updates_turn(&event, &receipt, &mut observed));
        assert!(!event_updates_turn(&event, &receipt, &mut observed));
        assert_eq!(observed.len(), 2);
    }

    #[test]
    fn human_progress_prints_one_line_for_tool_lifecycle_snapshots() {
        let receipt = HostCommandReceipt {
            attachment_ref: None,
            conversation_ref: "conversation".into(),
            message_ref: Some("request".into()),
            invocation_ref: Some("invocation".into()),
            stopped_invocation_refs: Vec::new(),
        };
        let conversation = serde_json::from_value(json!({
            "conversation_ref": "conversation",
            "participant_actor_refs": [],
            "messages": [],
            "activity": [
                {
                    "activity_ref": "started-snapshot",
                    "sequence": 1,
                    "event": {
                        "kind": "work_update",
                        "invocation_ref": "invocation",
                        "source_message_ref": "request",
                        "actor_ref": "~@/actors/helper",
                        "status": 102,
                        "activity": {
                            "activity_ref": "tool-operation",
                            "kind": "command",
                            "label": "Searching the workspace",
                            "state": "running",
                            "started_at_unix_ms": 1
                        }
                    }
                },
                {
                    "activity_ref": "completed-snapshot",
                    "sequence": 2,
                    "event": {
                        "kind": "work_update",
                        "invocation_ref": "invocation",
                        "source_message_ref": "request",
                        "actor_ref": "~@/actors/helper",
                        "status": 102,
                        "activity": {
                            "activity_ref": "tool-operation",
                            "kind": "command",
                            "label": "Searching the workspace",
                            "state": "completed",
                            "started_at_unix_ms": 1,
                            "duration_ms": 2
                        }
                    }
                }
            ]
        }))
        .unwrap();
        let event = HostEvent::ConversationUpsert {
            source_ref: "source".into(),
            conversation,
        };
        let mut observed = std::collections::BTreeSet::new();

        assert_eq!(
            turn_progress_lines(&event, &receipt, &mut observed),
            vec!["Searching the workspace"]
        );
        assert_eq!(observed.len(), 1);
    }

    #[test]
    fn human_progress_includes_descendant_action_updates() {
        let receipt = HostCommandReceipt {
            attachment_ref: None,
            conversation_ref: "conversation".into(),
            message_ref: Some("request".into()),
            invocation_ref: Some("root".into()),
            stopped_invocation_refs: Vec::new(),
        };
        let conversation = serde_json::from_value(json!({
            "conversation_ref": "conversation",
            "participant_actor_refs": [],
            "messages": [],
            "activity": [
                {
                    "activity_ref": "child-link",
                    "sequence": 1,
                    "event": {
                        "kind": "work_update",
                        "invocation_ref": "child",
                        "source_message_ref": "child-call",
                        "parent_invocation_ref": "root",
                        "actor_ref": "~@/actors/router",
                        "status": 200,
                        "text": "Child finished."
                    }
                },
                {
                    "activity_ref": "grandchild-update",
                    "sequence": 2,
                    "event": {
                        "kind": "work_update",
                        "invocation_ref": "grandchild",
                        "source_message_ref": "grandchild-call",
                        "parent_invocation_ref": "child",
                        "actor_ref": "~@/actors/checks",
                        "status": 102,
                        "text": "Five of six checks passed."
                    }
                }
            ]
        }))
        .unwrap();
        let event = HostEvent::ConversationUpsert {
            source_ref: "source".into(),
            conversation,
        };
        let mut observed = std::collections::BTreeSet::new();

        assert_eq!(
            turn_progress_lines(&event, &receipt, &mut observed),
            vec!["Five of six checks passed."]
        );
        assert!(event_updates_turn(
            &event,
            &receipt,
            &mut std::collections::BTreeSet::new()
        ));
    }

    #[test]
    fn terminal_event_preserves_the_authoritative_action_response() {
        let response = ActionResponse {
            progress: None,
            invocation_ref: "cwp://example.test/invocations/one".into(),
            sequence: 7,
            status: ActionResponseStatus::COMPLETED,
            summary: "Finished".into(),
            result: Some(json!({"value": 42})),
            error: None,
            cause: None,
        };
        let event = HostEvent::ConversationExecutionFinished {
            source_ref: "cwp://example.test/sources/local".into(),
            conversation_ref: "cwp://example.test/conversations/one".into(),
            target_actor_ref: "~@/actors/helper".into(),
            response: Box::new(response.clone()),
        };

        let (_, actor_ref, summary, observed) = terminal_update(
            &event,
            "cwp://example.test/conversations/one",
            &response.invocation_ref,
        )
        .expect("observe matching terminal");

        assert_eq!(actor_ref, "~@/actors/helper");
        assert_eq!(summary.as_deref(), Some("Finished"));
        assert_eq!(observed, Some(response));
    }

    #[test]
    fn execution_failure_terminates_the_matching_wait() {
        let event = HostEvent::ConversationExecutionFailed {
            source_ref: "cwp://example.test/sources/local".into(),
            conversation_ref: "cwp://example.test/conversations/one".into(),
            invocation_ref: "cwp://example.test/invocations/one".into(),
            target_actor_ref: "~@/actors/helper".into(),
            error: "could not persist terminal state".into(),
        };

        let (status, _, summary, response) = terminal_update(
            &event,
            "cwp://example.test/conversations/one",
            "cwp://example.test/invocations/one",
        )
        .expect("observe matching execution failure");

        assert_eq!(status, ActionResponseStatus::FAILED);
        assert_eq!(summary.as_deref(), Some("could not persist terminal state"));
        assert!(response.is_none());
    }
}
