#[test]
fn coworker_calls_keep_one_child_link_without_duplicate_parent_progress() {
    let root: RetainedConversation = serde_json::from_value(json!({
        "conversation_ref": "cwp://example.test/conversations/root",
        "participant_actor_refs": [
            crate::v2026_09::test_fixture::actor_ref("person"),
            "~@/actors/parent",
            "~@/actors/child"
        ],
        "messages": [{
            "message_ref": "cwp://example.test/messages/request",
            "author_actor_ref": crate::v2026_09::test_fixture::actor_ref("person"),
            "addressed_actor_refs": ["~@/actors/parent"],
            "text": "Coordinate a review."
        }, {
            "message_ref": "cwp://example.test/messages/child-call",
            "author_actor_ref": "~@/actors/parent",
            "addressed_actor_refs": ["~@/actors/child"],
            "text": "@child--review--projection",
            "action": {
                "kind": "call",
                "activation": "@child--review--projection",
                "input": {}
            }
        }]
    }))
    .unwrap();
    let parent: RetainedConversation = serde_json::from_value(json!({
        "conversation_ref": "cwp://example.test/conversations/parent-execution",
        "parent_conversation_ref": root.conversation_ref,
        "participant_actor_refs": ["~@/actors/parent"],
        "messages": [],
        "activity": [{
            "activity_ref": "cwp://example.test/activities/parent-start",
            "sequence": 1,
            "occurred_at_unix_ms": 1,
            "event": {
                "kind": "turn_started",
                "invocation_ref": "cwp://example.test/invocations/parent",
                "source_message_ref": "cwp://example.test/messages/request"
            }
        }, {
            "activity_ref": "cwp://example.test/activities/host-call",
            "sequence": 2,
            "event": {"kind": "host_call", "call_id": "review",
                "operation": "cwr.invoke_action",
                "input": {"activation": "@child--review--projection"}}
        }, {
            "activity_ref": "cwp://example.test/activities/child-start",
            "sequence": 2,
            "occurred_at_unix_ms": 2,
            "event": {
                "kind": "host_action_started",
                "call_id": "review",
                "nested_invocation_ref": "cwp://example.test/invocations/child",
                "call_message_ref": "cwp://example.test/messages/child-call"
            }
        }, {
            "activity_ref": "cwp://example.test/activities/child-result",
            "sequence": 3,
            "occurred_at_unix_ms": 12,
            "event": {
                "kind": "host_result",
                "call_id": "review",
                "error": "Review service was unavailable."
            }
        }]
    }))
    .unwrap();
    let child: RetainedConversation = serde_json::from_value(json!({
        "conversation_ref": "cwp://example.test/conversations/child-execution",
        "parent_conversation_ref": root.conversation_ref,
        "participant_actor_refs": ["~@/actors/child"],
        "messages": [],
        "activity": [
            {
                "activity_ref": "cwp://example.test/activities/child-turn-start",
                "sequence": 1,
                "occurred_at_unix_ms": 3,
                "event": {
                    "kind": "turn_started",
                    "invocation_ref": "cwp://example.test/invocations/child",
                    "source_message_ref": "cwp://example.test/messages/child-call"
                }
            },
            {
                "activity_ref": "cwp://example.test/activities/child-progress",
                "sequence": 2,
                "occurred_at_unix_ms": 4,
                "event": {
                    "kind": "progress",
                    "status": 102,
                    "summary": "Five of six checks passed."
                }
            },
            {
                "activity_ref": "cwp://example.test/activities/child-commentary",
                "sequence": 3,
                "occurred_at_unix_ms": 5,
                "event": {
                    "kind": "commentary_progress", "model_run_ref": "model",
                    "provider_integration_ref": "provider", "provider": "codex",
                    "response_or_turn_id": "turn", "item_id": "comment",
                    "text": "The final check needs the review service."
                }
            },
            {
                "activity_ref": "cwp://example.test/activities/child-failed",
                "sequence": 4,
                "occurred_at_unix_ms": 11,
                "event": {
                    "kind": "turn_failed", "status": 502,
                    "error": "Review service was unavailable."
                }
            }
        ]
    }))
    .unwrap();

    let projected = project_conversation_with_work(&root, &[root.clone(), parent, child]);
    let child_actions = projected
        .activity
        .iter()
        .filter_map(|item| match &item.event {
            ConversationActivityEvent::WorkUpdate {
                invocation_ref,
                activity: Some(activity),
                ..
            } if activity.child_invocation_ref.is_some() => Some((invocation_ref, activity)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        child_actions.len(),
        1,
        "one relationship connects the nested work card"
    );
    assert_eq!(child_actions[0].0, "cwp://example.test/invocations/parent");
    assert_eq!(
        child_actions[0].1.activity_ref,
        "cwp://example.test/activities/host-call"
    );
    assert_eq!(
        child_actions[0].1.child_invocation_ref.as_deref(),
        Some("cwp://example.test/invocations/child")
    );
    assert_eq!(child_actions[0].1.state, "running");
    assert_eq!(
        projected
            .activity
            .iter()
            .filter(|item| matches!(
                &item.event,
                ConversationActivityEvent::WorkUpdate { invocation_ref, activity: Some(_), .. }
                    if invocation_ref == "cwp://example.test/invocations/parent"
            ))
            .count(),
        1,
        "an admitted call's host error must not create a duplicate activity"
    );
    assert!(projected
        .messages
        .iter()
        .any(|message| message.message_ref == "cwp://example.test/messages/child-call"));
    let child_parents = projected
        .activity
        .iter()
        .filter_map(|item| match &item.event {
            ConversationActivityEvent::WorkUpdate {
                invocation_ref,
                parent_invocation_ref,
                ..
            } if invocation_ref == "cwp://example.test/invocations/child" => {
                Some(parent_invocation_ref.as_deref())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        child_parents,
        [
            Some("cwp://example.test/invocations/parent"),
            Some("cwp://example.test/invocations/parent"),
            Some("cwp://example.test/invocations/parent"),
            Some("cwp://example.test/invocations/parent")
        ]
    );
    assert_child_updates(&projected);
    HostEvent::ConversationUpsert {
        source_ref: "cwp://example.test/sources/local".into(),
        conversation: projected,
    }
    .validate()
    .unwrap();
}

fn assert_child_updates(projected: &RetainedConversation) {
    assert!(projected.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::WorkUpdate {
            invocation_ref,
            status,
            text: Some(text),
            ..
        } if invocation_ref == "cwp://example.test/invocations/child"
            && status.as_u16() == 102
            && text == "Five of six checks passed."
    )));
    assert!(projected.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::WorkUpdate {
            invocation_ref, text: Some(text), text_kind: Some(crate::v2026_09::conversation::WorkUpdateTextKind::Commentary), ..
        } if invocation_ref == "cwp://example.test/invocations/child"
            && text == "The final check needs the review service."
    )));
    assert!(projected.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::WorkUpdate { invocation_ref, status, text: Some(text), .. }
            if invocation_ref == "cwp://example.test/invocations/child"
                && status.as_u16() == 502 && text == "Review service was unavailable."
    )));
}
