#[test]
fn message_calls_link_child_work_and_preserve_ordinary_messages_on_replay() {
    let root: RetainedConversation = serde_json::from_value(json!({
        "conversation_ref":"cwp://example.test/conversations/root",
        "participant_actor_refs":["~@/actors/parent", "~@/actors/child"],
        "messages":[
            {"message_ref":"request", "author_actor_ref":"~@/actors/person",
                "addressed_actor_refs":["~@/actors/parent"], "text":"Ask for a review."},
            {"message_ref":"child-request", "author_actor_ref":"~@/actors/parent",
                "addressed_actor_refs":["~@/actors/child"], "text":"Please review this."},
            {"message_ref":"child-reply", "author_actor_ref":"~@/actors/child",
                "addressed_actor_refs":["~@/actors/parent"], "reply_to_message_ref":"child-request",
                "text":"The changes look good."}
        ]
    }))
    .unwrap();
    let parent: RetainedConversation = serde_json::from_value(json!({
        "conversation_ref":"cwp://example.test/conversations/parent-execution",
        "parent_conversation_ref":root.conversation_ref,
        "participant_actor_refs":["~@/actors/parent"], "messages":[],
        "activity":[
            {"activity_ref":"parent-start", "sequence":1, "event":{
                "kind":"turn_started", "invocation_ref":"parent", "source_message_ref":"request"}},
            {"activity_ref":"host-call", "sequence":2, "event":{
                "kind":"host_call", "call_id":"review", "operation":"cwr.send_message",
                "input":{"to":"@child", "text":"Please review this."}}},
            {"activity_ref":"child-start", "sequence":3, "event":{
                "kind":"host_action_started", "call_id":"review",
                "nested_invocation_ref":"child", "call_message_ref":"child-request"}},
            {"activity_ref":"host-result", "sequence":4, "event":{
                "kind":"host_result", "call_id":"review", "result":{"status":200}}}
        ]
    }))
    .unwrap();
    let child: RetainedConversation = serde_json::from_value(json!({
        "conversation_ref":"cwp://example.test/conversations/child-execution",
        "parent_conversation_ref":root.conversation_ref,
        "participant_actor_refs":["~@/actors/child"], "messages":[],
        "activity":[
            {"activity_ref":"child-turn-start", "sequence":1, "event":{
                "kind":"turn_started", "invocation_ref":"child", "source_message_ref":"child-request"}},
            {"activity_ref":"child-commentary", "sequence":2, "event":{
                "kind":"commentary_progress", "model_run_ref":"model", "provider_integration_ref":"provider",
                "provider":"codex", "response_or_turn_id":"turn", "item_id":"commentary",
                "text":"I am checking the edge cases."}}
        ]
    })).unwrap();
    for count in [3, 4, 4] {
        let mut parent = parent.clone();
        parent.activity.truncate(count);
        let projected = project_conversation_with_work(&root, &[parent, child.clone()]);
        assert_eq!(projected.messages, root.messages);
        assert!(projected
            .messages
            .iter()
            .all(|message| message.action.is_none()));
        let links: Vec<_> = projected
            .activity
            .iter()
            .filter_map(|item| match &item.event {
                ConversationActivityEvent::WorkUpdate {
                    activity: Some(activity),
                    ..
                } => Some(activity),
                _ => None,
            })
            .collect();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].activity_ref, "host-call");
        assert_eq!(links[0].child_invocation_ref.as_deref(), Some("child"));
        assert_eq!(links[0].label, "Calling @child");
        assert!(projected.activity.iter().any(|item| matches!(&item.event,
            ConversationActivityEvent::WorkUpdate {
                invocation_ref, parent_invocation_ref: Some(parent), text: Some(text), ..
            } if invocation_ref == "child" && parent == "parent" && text == "I am checking the edge cases.")));
    }
}
