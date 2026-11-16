#[test]
fn authoritative_timeline_preserves_synchronous_lifecycle_parent() {
    let root: RetainedConversation = serde_json::from_value(serde_json::json!({
        "conversation_ref": "cwp://example.test/conversations/root",
        "participant_actor_refs": ["~@/actors/person", "~@/actors/parent", "~@/actors/child"],
        "messages": [{
            "message_ref": "cwp://example.test/messages/request",
            "author_actor_ref": "~@/actors/person", "addressed_actor_refs": ["~@/actors/parent"],
            "text": "Help"
        }]
    }))
    .unwrap();
    let parent_execution: RetainedConversation = serde_json::from_value(serde_json::json!({
        "conversation_ref": "cwp://example.test/conversations/parent-execution",
        "parent_conversation_ref": root.conversation_ref,
        "participant_actor_refs": ["~@/actors/parent"],
        "messages": [],
        "activity": [{
            "activity_ref": "cwp://example.test/activities/parent-start", "sequence": 1,
            "event": {"kind": "turn_started",
                "invocation_ref": "cwp://example.test/invocations/parent",
                "source_message_ref": "cwp://example.test/messages/request"}
        }, {
            "activity_ref": "cwp://example.test/activities/send-child", "sequence": 2,
            "event": {"kind": "host_call", "call_id": "send-child",
                "operation": "cwr.send_message", "input": {}}
        }]
    }))
    .unwrap();
    let child_execution: RetainedConversation = serde_json::from_value(serde_json::json!({
        "conversation_ref": "cwp://example.test/conversations/child-execution",
        "parent_conversation_ref": root.conversation_ref,
        "participant_actor_refs": ["~@/actors/child"],
        "messages": [],
        "activity": [{
            "activity_ref": "cwp://example.test/activities/child-start", "sequence": 1,
            "event": {"kind": "turn_started",
                "invocation_ref": "cwp://example.test/invocations/child",
                "source_message_ref": "cwp://example.test/messages/request"}
        }]
    }))
    .unwrap();
    let parent_ref = "cwp://example.test/invocations/parent";
    let child_ref = "cwp://example.test/invocations/child";
    let ledger = serde_json::from_value(serde_json::json!({
        "invocations": {
            parent_ref: {
                "invocation_ref": parent_ref,
                "origin": {"kind": "ingress", "integration_ref": "~@/integrations/test",
                    "provider_event_ref": "request"},
                "requester_actor_ref": "~@/actors/person", "target_actor_ref": "~@/actors/parent",
                "conversation_ref": root.conversation_ref, "action_name": "respond--message",
                "input": {}, "state": "running", "admitted_at_unix_ms": 1
            },
            child_ref: {
                "invocation_ref": child_ref,
                "origin": {"kind": "host_call", "caller_invocation_ref": parent_ref,
                    "call_id": "send-child"},
                "requester_actor_ref": "~@/actors/parent", "target_actor_ref": "~@/actors/child",
                "conversation_ref": root.conversation_ref, "action_name": "respond--message",
                "input": {}, "parent_invocation_ref": parent_ref, "state": "running",
                "admitted_at_unix_ms": 2
            }
        }
    }))
    .unwrap();
    let state = ConversationRuntimeState {
        protected_result_children: Default::default(),
        streaming_messages: Default::default(),
        streaming_runs: Default::default(),
        tool_attempts: Default::default(),
        tool_operations: Default::default(),
        version: STATE_VERSION.into(),
        runtime_root_ref: "cwp://example.test".into(),
        conversations: vec![root.clone(), parent_execution, child_execution].into(),
        cold_conversation_refs: Default::default(),
        files: Default::default(),
        visualization_artifacts: BTreeMap::new().into(),
        admissions: Vec::new().into(),
        invocation_ledger: ledger,
        timeline_store: ConversationTimelineStore::default(),
        invocation_execution_conversation_refs: BTreeMap::from([
            (
                parent_ref.into(),
                "cwp://example.test/conversations/parent-execution".into(),
            ),
            (
                child_ref.into(),
                "cwp://example.test/conversations/child-execution".into(),
            ),
        ])
        .into(),
        upstream_participant_versions: BTreeMap::new().into(),
    };

    let projected = authoritative_projection(&state, &root);
    let awaited_by = projected
        .activity
        .iter()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::WorkLifecycle {
                invocation_ref,
                awaited_by_invocation_ref,
                ..
            } if invocation_ref == child_ref => awaited_by_invocation_ref.as_deref(),
            _ => None,
        });

    assert_eq!(awaited_by, Some(parent_ref));
}

#[test]
fn recent_completed_work_uses_only_root_invocations() {
    let conversation: RetainedConversation = serde_json::from_value(serde_json::json!({
        "conversation_ref": "cwp://example.test/conversations/root",
        "participant_actor_refs": [],
        "messages": [],
        "activity": [{
            "activity_ref": "cwp://example.test/activities/older", "sequence": 1,
            "occurred_at_unix_ms": 10,
            "event": {"kind": "work_lifecycle",
                "invocation_ref": "cwp://example.test/invocations/older",
                "actor_ref": "~@/actors/worker",
                "source_message_ref": "cwp://example.test/messages/request",
                "parent_invocation_ref": null, "state": "completed",
                "admitted_at_unix_ms": 1, "started_at_unix_ms": 2,
                "stop_requested_at_unix_ms": null, "finished_at_unix_ms": 10}
        }, {
            "activity_ref": "cwp://example.test/activities/latest", "sequence": 2,
            "occurred_at_unix_ms": 20,
            "event": {"kind": "work_lifecycle",
                "invocation_ref": "cwp://example.test/invocations/latest",
                "actor_ref": "~@/actors/worker",
                "source_message_ref": "cwp://example.test/messages/request",
                "parent_invocation_ref": null, "state": "completed",
                "admitted_at_unix_ms": 11, "started_at_unix_ms": 12,
                "stop_requested_at_unix_ms": null, "finished_at_unix_ms": 20}
        }, {
            "activity_ref": "cwp://example.test/activities/child", "sequence": 3,
            "occurred_at_unix_ms": 30,
            "event": {"kind": "work_lifecycle",
                "invocation_ref": "cwp://example.test/invocations/child",
                "actor_ref": "~@/actors/worker",
                "source_message_ref": "cwp://example.test/messages/request",
                "parent_invocation_ref": "cwp://example.test/invocations/latest",
                "state": "completed", "admitted_at_unix_ms": 21,
                "started_at_unix_ms": 22, "stop_requested_at_unix_ms": null,
                "finished_at_unix_ms": 30}
        }]
    }))
    .unwrap();

    assert_eq!(
        recent_completed_root_work_refs(&conversation),
        vec![
            "cwp://example.test/invocations/older".to_string(),
            "cwp://example.test/invocations/latest".to_string(),
        ]
    );
}


