fn project_provider_call(events: Vec<Value>) -> RetainedConversation {
    let root: RetainedConversation = serde_json::from_value(json!({
        "conversation_ref":"cwp://example.test/conversations/root",
        "participant_actor_refs":["~@/actors/parent"],
        "messages":[{"message_ref":"request", "author_actor_ref":"~@/actors/person",
            "addressed_actor_refs":["~@/actors/parent"], "text":"Review the changes."}]
    }))
    .unwrap();
    let events = std::iter::once(json!({"kind":"turn_started", "invocation_ref":"parent",
        "source_message_ref":"request"}))
    .chain(events)
    .enumerate()
    .map(|(index, event)| {
        json!({"activity_ref":format!("activity-{index}"), "sequence":index+1,
            "occurred_at_unix_ms":index+1, "event":event})
    })
    .collect::<Vec<_>>();
    let execution: RetainedConversation = serde_json::from_value(json!({
        "conversation_ref":"cwp://example.test/conversations/execution",
        "parent_conversation_ref":root.conversation_ref,
        "participant_actor_refs":["~@/actors/parent"], "messages":[], "activity":events
    }))
    .unwrap();
    project_conversation_with_work(&root, &[execution])
}

fn provider_tool_event(method: &str, item_type: &str, id: &str, tool: &str, failed: bool) -> Value {
    json!({"kind":"provider_event", "provider":"codex", "method":method,
        "data":{"item":{"id":id, "type":item_type, "namespace":"cwp", "tool":tool,
            "status":if failed { "failed" } else if method == "item/started" { "inProgress" } else { "completed" },
            "error":if failed { json!({"message":"Request rejected"}) } else { Value::Null }}}})
}

fn public_tool_activities(projected: &RetainedConversation) -> Vec<&WorkActivity> {
    projected
        .activity
        .iter()
        .filter_map(|item| match &item.event {
            ConversationActivityEvent::WorkUpdate {
                activity: Some(activity),
                ..
            } => Some(activity.as_ref()),
            _ => None,
        })
        .collect()
}

#[test]
fn native_and_generic_provider_calls_share_the_host_child_link_on_replay() {
    for (item_type, tool) in [
        ("dynamicToolCall", "cwr--invoke--action"),
        ("dynamicToolCall", "sourceng_ops--inspect--changes"),
        ("mcpToolCall", "cwr--invoke--action"),
    ] {
        let id = "call_3HCs4UK1rkso5H6CLin5GQIS";
        let events = [
            provider_tool_event("item/started", item_type, id, tool, false),
            json!({"kind":"provider_event", "provider":"codex", "method":"item/tool/call",
                "data":{"call_id":id, "namespace":"cwp", "tool":tool, "arguments":{}}}),
            json!({"kind":"host_call", "call_id":id, "operation":"cwr.invoke_action",
                "input":{"activation":"@sourceng_ops--inspect--changes", "input":{}}}),
            json!({"kind":"host_action_started", "call_id":id,
                "nested_invocation_ref":"child", "call_message_ref":"child-call"}),
            json!({"kind":"host_result", "call_id":id, "result":{"status":200}}),
            provider_tool_event("item/completed", item_type, id, tool, false),
        ];
        for count in [4, 5, 6, 6] {
            let projected = project_provider_call(events[..count].to_vec());
            let activities = public_tool_activities(&projected);
            assert_eq!(activities.len(), 1, "{item_type}: {tool}");
            assert_eq!(activities[0].kind, "coworker");
            assert_eq!(activities[0].child_invocation_ref.as_deref(), Some("child"));
            assert_eq!(activities[0].activity_ref, "activity-3");
        }
    }
}

#[test]
fn correlated_pre_admission_error_remains_visible_without_its_provider_duplicate() {
    let projected = project_provider_call(vec![
        provider_tool_event(
            "item/started",
            "dynamicToolCall",
            "denied",
            "cwr--send--message",
            false,
        ),
        json!({"kind":"host_call", "call_id":"denied", "operation":"cwr.send_message", "input":{"to":"@helper"}}),
        json!({"kind":"host_result", "call_id":"denied", "error":"Coworker request is not authorized"}),
        provider_tool_event(
            "item/completed",
            "dynamicToolCall",
            "denied",
            "cwr--send--message",
            true,
        ),
    ]);
    let activities = public_tool_activities(&projected);
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].state, "failed");
    assert_eq!(activities[0].child_invocation_ref, None);
    assert_eq!(
        activities[0].error.as_deref(),
        Some("Coworker request is not authorized")
    );
}

#[test]
fn provider_failures_without_host_admission_and_unrelated_tools_are_preserved() {
    let projected = project_provider_call(vec![
        provider_tool_event(
            "item/started",
            "dynamicToolCall",
            "invalid",
            "cwr--invoke--action",
            false,
        ),
        provider_tool_event(
            "item/completed",
            "dynamicToolCall",
            "invalid",
            "cwr--invoke--action",
            true,
        ),
        provider_tool_event(
            "item/started",
            "dynamicToolCall",
            "discovery",
            "cwr--list--actions",
            false,
        ),
        json!({"kind":"host_call", "call_id":"discovery", "operation":"cwr.list_actions", "input":{}}),
        json!({"kind":"host_result", "call_id":"discovery", "error":"Discovery unavailable"}),
        provider_tool_event(
            "item/completed",
            "dynamicToolCall",
            "discovery",
            "cwr--list--actions",
            true,
        ),
        provider_tool_event(
            "item/completed",
            "mcpToolCall",
            "issue",
            "read_issue",
            false,
        ),
    ]);
    let activities = public_tool_activities(&projected);
    assert_eq!(activities.len(), 5);
    assert!(activities
        .iter()
        .any(|activity| activity.label == "Using read_issue"));
    assert_eq!(
        activities
            .iter()
            .filter(|activity| activity.state == "failed")
            .count(),
        2
    );
    assert!(activities.iter().all(|activity| activity.kind == "tool"));
}
