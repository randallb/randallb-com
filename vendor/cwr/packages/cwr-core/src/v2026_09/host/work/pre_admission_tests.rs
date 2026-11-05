mod pre_admission_work_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn coworker_failure_before_admission_is_projected_from_the_host_call() {
        let root: RetainedConversation = serde_json::from_value(json!({
            "conversation_ref": "cwp://example.test/conversations/root",
            "participant_actor_refs": [crate::v2026_09::test_fixture::actor_ref("person"), "~@/actors/parent"],
            "messages": [{
                "message_ref": "cwp://example.test/messages/request",
                "author_actor_ref": crate::v2026_09::test_fixture::actor_ref("person"),
                "addressed_actor_refs": ["~@/actors/parent"],
                "text": "Coordinate a review."
            }]
        }))
        .unwrap();
        let execution: RetainedConversation = serde_json::from_value(json!({
            "conversation_ref": "cwp://example.test/conversations/execution",
            "parent_conversation_ref": root.conversation_ref,
            "participant_actor_refs": ["~@/actors/parent"],
            "messages": [],
            "activity": [{
                "activity_ref": "cwp://example.test/activities/start",
                "sequence": 1,
                "occurred_at_unix_ms": 1,
                "event": {
                    "kind": "turn_started",
                    "invocation_ref": "cwp://example.test/invocations/parent",
                    "source_message_ref": "cwp://example.test/messages/request"
                }
            }, {
                "activity_ref": "cwp://example.test/activities/call",
                "sequence": 2,
                "occurred_at_unix_ms": 2,
                "event": {
                    "kind": "host_call",
                    "call_id": "review",
                    "operation": "cwr.invoke_action",
                    "input": {
                        "activation": "@reviewer--review--projection",
                        "input": {"secret": "private"}
                    }
                }
            }, {
                "activity_ref": "cwp://example.test/activities/result",
                "sequence": 3,
                "occurred_at_unix_ms": 12,
                "event": {
                    "kind": "host_result",
                    "call_id": "review",
                    "error": "action @reviewer--review--projection is not visible"
                }
            }]
        }))
        .unwrap();

        let projected = project_conversation_with_work(&root, &[root.clone(), execution]);
        let activity = projected.activity.iter().find_map(|item| match &item.event {
            ConversationActivityEvent::WorkUpdate {
                activity: Some(activity),
                ..
            } if activity.state == "failed" => Some(activity),
            _ => None,
        });

        let activity = activity.expect("failed coworker call should be projected");
        assert_eq!(activity.kind, "coworker");
        assert_eq!(activity.label, "Calling @reviewer--review--projection");
        assert_eq!(activity.started_at_unix_ms, 2);
        assert_eq!(activity.duration_ms, Some(10));
        assert_eq!(
            activity.error.as_deref(),
            Some("action @reviewer--review--projection is not visible")
        );
        let wire = serde_json::to_string(&projected).unwrap();
        assert!(!wire.contains("secret"));
        assert!(!wire.contains("private"));
    }
}
