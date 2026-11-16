#[test]
fn runtime_cwr_actions_are_visible_to_executing_coworkers() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "@helper coordinate").unwrap();
    let state = runtime.lock().unwrap();

    let routes = visible_action_routes(
        &state,
        admission.conversation_ref(),
        crate::v2026_09::test_fixture::actor_ref("helper"),
        None,
    )
    .unwrap();

    assert!(!routes.contains_key("@cwr--reload--graph"));
    assert!(routes.contains_key("@cwr--debug--invocation"));
    assert!(routes.contains_key("@cwr--stop--invocation"));
    assert!(routes.contains_key("@cwr--list--conversations"));
    assert!(routes.contains_key("@cwr--read--conversation"));
    assert_eq!(
        routes["@cwr--list--conversations"].input_schema["properties"]["archived"]["type"],
        "boolean"
    );
    let list_route = &routes["@cwr--list--conversations"];
    let list_input = jsonschema::validator_for(&list_route.input_schema).unwrap();
    assert!(!list_input.is_valid(&json!({"page": {}})));
    assert!(list_input.is_valid(&json!({"page": {"first": 10}, "archived": false})));
    let list_output = jsonschema::validator_for(&list_route.output_schema).unwrap();
    let listed = serde_json::to_value(IntrinsicCwrResponse::ConversationsListed {
        response: ConversationListResponse {
            page: ConversationSummaryPage {
                membership_revision: "members-1".into(),
                revision: "1".into(),
                summaries: vec![crate::v2026_09::conversation::ConversationSummary {
                    completed_work: vec![],
                    key: crate::v2026_09::conversation::ConversationSummaryKey {
                        creation_order: 1,
                        conversation_ref: "cwp://example.test/conversations/one".into(),
                    },
                    title: Some("One".into()),
                    archived: false,
                    last_message_ref: Some("cwp://example.test/messages/one".into()),
                    last_message_preview: Some("hello".into()),
                    completed_work_refs: Vec::new(),
                    revision: "1".into(),
                }],
                has_next_page: false,
                has_previous_page: false,
                total_count: 0,
                archived_count: 0,
            },
        },
    })
    .unwrap();
    assert!(list_output.is_valid(&listed));
    assert!(!list_output.is_valid(&json!({"kind": "conversations_listed", "page": {}})));

    let read_route = &routes["@cwr--read--conversation"];
    let read_input = jsonschema::validator_for(&read_route.input_schema).unwrap();
    assert!(!read_input.is_valid(&json!({"page": {}})));
    assert!(read_input.is_valid(&json!({"page": {"last": 10}})));
    let browse_route = &routes["@cwr--browse--conversation"];
    let browse_input = jsonschema::validator_for(&browse_route.input_schema).unwrap();
    assert!(!browse_input.is_valid(&json!({"operation": {"kind": "recent", "page": {}}})));
    for request in [
        json!({"conversation_ref": "", "operation": {"kind": "recent", "page": {"last": 1}}}),
        json!({"operation": {"kind": "around", "message_ref": ""}}),
        json!({"operation": {"kind": "excerpt", "message_ref": ""}}),
        json!({"operation": {"kind": "links", "message_ref": ""}}),
        json!({"operation": {"kind": "links", "message_ref": "cwp://example.test/messages/one", "after_reply_message_ref": ""}}),
    ] {
        assert!(
            !browse_input.is_valid(&request),
            "browse schema accepted {request}"
        );
    }
    for bytes in 1..=4 {
        for operation in [
            json!({"kind": "recent", "page": {"last": 1}, "excerpt_bytes": bytes}),
            json!({"kind": "around", "message_ref": "cwp://example.test/messages/one", "excerpt_bytes": bytes}),
            json!({"kind": "search", "query": "é", "excerpt_bytes": bytes}),
            json!({"kind": "excerpt", "message_ref": "cwp://example.test/messages/one", "max_bytes": bytes}),
        ] {
            assert_eq!(
                browse_input.is_valid(&json!({"operation": operation})),
                bytes == 4
            );
        }
    }
    for operation in [
        json!({"kind": "search", "query": ""}),
        json!({"kind": "search", "query": "   "}),
        json!({"kind": "search", "query": "project code", "limit": 0}),
        json!({"kind": "search", "query": "project code", "limit": 26}),
        json!({"kind": "links", "message_ref": "cwp://example.test/messages/one", "limit": 0}),
        json!({"kind": "recent", "page": {"last": 1}, "excerpt_bytes": 2049}),
        json!({"kind": "recent", "page": {"last": 1}, "excerpt_bytes": 0}),
        json!({"kind": "search", "query": "project code", "excerpt_bytes": 2049}),
        json!({"kind": "search", "query": "project code", "excerpt_bytes": 0}),
        json!({"kind": "around", "message_ref": "cwp://example.test/messages/one", "excerpt_bytes": 2049}),
        json!({"kind": "around", "message_ref": "cwp://example.test/messages/one", "excerpt_bytes": 0}),
        json!({"kind": "excerpt", "message_ref": "cwp://example.test/messages/one", "max_bytes": 0}),
        json!({"kind": "excerpt", "message_ref": "cwp://example.test/messages/one", "max_bytes": 2049}),
        json!({"kind": "around", "message_ref": "cwp://example.test/messages/one", "before": 25, "after": 0}),
        json!({"kind": "around", "message_ref": "cwp://example.test/messages/one", "before": 20, "after": 5}),
        json!({"kind": "around", "message_ref": "cwp://example.test/messages/one", "before": 15}),
        json!({"kind": "around", "message_ref": "cwp://example.test/messages/one", "after": 15}),
    ] {
        assert!(
            !browse_input.is_valid(&json!({"operation": operation})),
            "browse schema accepted invalid operation {operation}"
        );
    }
    for request in [
        json!({"operation": {"kind": "recent", "page": {"last": 10}}}),
        json!({"conversation_ref": null, "operation": {"kind": "recent", "page": {"last": 1}}}),
        json!({"operation": {"kind": "search", "query": "project code"}}),
        json!({"operation": {"kind": "search", "query": "Éclair", "limit": 25}}),
        json!({"operation": {"kind": "search", "query": "é".repeat(200)}}),
        json!({"operation": {"kind": "around", "message_ref": "cwp://example.test/messages/one", "before": 24, "after": 0}}),
        json!({"operation": {"kind": "around", "message_ref": "cwp://example.test/messages/one", "before": 14, "after": 10}}),
        json!({"operation": {"kind": "around", "message_ref": "cwp://example.test/messages/one"}}),
        json!({"operation": {"kind": "excerpt", "message_ref": "cwp://example.test/messages/one"}}),
    ] {
        assert!(
            browse_input.is_valid(&request),
            "browse schema rejected {request}"
        );
        serde_json::from_value::<crate::intrinsic_cwr::ConversationBrowseRequest>(request).unwrap();
    }
    let links_without_cursor = serde_json::from_value::<crate::intrinsic_cwr::ConversationBrowseRequest>(json!({
        "conversation_ref": null,
        "operation": {"kind": "links", "message_ref": "cwp://example.test/messages/one", "after_reply_message_ref": null}
    }))
    .unwrap();
    assert!(browse_input.is_valid(&serde_json::to_value(links_without_cursor).unwrap()));
    let read_output = jsonschema::validator_for(&read_route.output_schema).unwrap();
    let read = populated_read_response();
    assert!(read_output.is_valid(&read));
    let mut invalid_status = read.clone();
    invalid_status["records"]["page"]["activity"][3]["event"]["status"] = json!(350);
    assert!(!read_output.is_valid(&invalid_status));
    let mut invalid_progress = read.clone();
    invalid_progress["records"]["page"]["activity"][3]["event"]["progress"]["key"] =
        json!("bad\nkey");
    assert!(!read_output.is_valid(&invalid_progress));
    let mut invalid_lifecycle = read.clone();
    invalid_lifecycle["records"]["page"]["activity"][0]["event"] = json!({
        "kind": "work_lifecycle",
        "invocation_ref": "cwp://example.test/invocations/one",
        "actor_ref": crate::v2026_09::test_fixture::actor_ref("helper"),
        "source_message_ref": "cwp://example.test/messages/one",
        "parent_invocation_ref": null,
        "state": "unknown",
        "admitted_at_unix_ms": 1,
        "started_at_unix_ms": null,
        "stop_requested_at_unix_ms": null,
        "finished_at_unix_ms": null
    });
    assert!(!read_output.is_valid(&invalid_lifecycle));
    let mut invalid_null_action = read.clone();
    invalid_null_action["records"]["page"]["messages"][0]["action"] = Value::Null;
    assert!(!read_output.is_valid(&invalid_null_action));
    for event in representative_activity_events() {
        let event = serde_json::from_value::<ConversationActivityEvent>(event).unwrap();
        let mut response = read.clone();
        response["records"]["page"]["activity"][0]["event"] = serde_json::to_value(event).unwrap();
        assert!(read_output.is_valid(&response), "response: {response}");
    }
    assert!(!read_output.is_valid(&json!({
        "kind": "conversation_read",
        "conversation_ref": "cwp://example.test/conversations/one",
        "records": {}
    })));
    assert!(!read_output.is_valid(&json!({
        "kind": "conversation_read",
        "conversation_ref": "cwp://example.test/conversations/one",
        "records": {"kind": "content", "page": {
            "content": [], "has_next_page": false, "has_previous_page": false,
            "total_count": 0, "revision": "0"
        }}
    })));
    assert!(routes.contains_key("@cwr--rename--conversation"));
    assert!(routes.contains_key("@cwr--archive--conversation"));
    assert!(routes.contains_key("@cwr--restore--conversation"));
}

fn representative_activity_events() -> Vec<Value> {
    let invocation_ref = "cwp://example.test/invocations/one";
    let message_ref = "cwp://example.test/messages/one";
    let actor_ref = crate::v2026_09::test_fixture::actor_ref("helper");
    vec![
        json!({"kind":"message_recorded","message_ref":message_ref}),
        json!({"kind":"execution_activity","activity":{"phase":"using_tool","state":"updated"}}),
        json!({"kind":"work_phase","invocation_ref":invocation_ref,"actor_ref":actor_ref,
            "activity":{"phase":"working","state":"started"}}),
        json!({"kind":"work_lifecycle","invocation_ref":invocation_ref,"actor_ref":actor_ref,
            "source_message_ref":message_ref,"parent_invocation_ref":null,"state":"running",
            "admitted_at_unix_ms":1,"started_at_unix_ms":1,
            "stop_requested_at_unix_ms":null,"finished_at_unix_ms":null}),
        json!({"kind":"work_update","invocation_ref":invocation_ref,
            "source_message_ref":message_ref,"actor_ref":actor_ref,"status":102}),
        json!({"kind":"turn_started","invocation_ref":invocation_ref,
            "source_message_ref":message_ref}),
        json!({"kind":"provider_thread","provider":"fixture","thread_id":"thread-1"}),
        json!({"kind":"provider_event","provider":"fixture","method":"event","data":{}}),
        json!({"kind":"progress","status":102,"summary":"Working"}),
        json!({"kind":"commentary_progress","model_run_ref":"run-1",
            "provider_integration_ref":"integration-1","provider":"fixture",
            "response_or_turn_id":"turn-1","item_id":"item-1","text":"Working"}),
        json!({"kind":"host_call","call_id":"call-1","operation":"read","input":{}}),
        json!({"kind":"host_action_started","call_id":"call-1",
            "nested_invocation_ref":invocation_ref,"call_message_ref":message_ref}),
        json!({"kind":"host_result","call_id":"call-1","result":{}}),
        json!({"kind":"turn_completed","status":200,"summary":"Done","result":{}}),
        json!({"kind":"turn_attempt_failed","error":"retry"}),
        json!({"kind":"turn_failed","status":500,"error":"failed"}),
        json!({"kind":"turn_stopped","status":499,"reason":"stopped"}),
    ]
}

fn populated_read_response() -> Value {
    let message_ref = "cwp://example.test/messages/one".to_owned();
    serde_json::to_value(IntrinsicCwrResponse::ConversationRead {
        response: ConversationReadResponse {
            conversation_ref: "cwp://example.test/conversations/one".into(),
            records: ConversationReadRecords::Timeline {
                page: crate::v2026_09::conversation::ConversationTimelinePage {
                anchors: vec![crate::v2026_09::conversation::ConversationTimelineAnchor::Message {
                    key: crate::v2026_09::conversation::ConversationPositionKey {
                        position: 1,
                        revision: Some("1".into()),
                    },
                    message_ref: message_ref.clone(),
                }],
                messages: vec![ConversationMessage {
    file_refs: Vec::new(),
                    progress: None,
                    message_ref: message_ref.clone(),
                    author_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
                    addressed_actor_refs: Vec::new(),
                    addressing_mode: Some(
                        crate::v2026_09::conversation::ConversationAddressingMode::Implicit,
                    ),
                    text: "hello".into(),
                    visualizations: Vec::new(),
                    reply_to_message_ref: None,
                    action: Some(crate::v2026_09::conversation::ConversationActionEvent::Call {
                        activation: "@cwr--show--help".into(),
                        input: json!({}),
                    }),
                }],
                activity: vec![
                    ConversationActivity {
                        activity_ref: "cwp://example.test/activities/one".into(),
                        sequence: 1,
                        occurred_at_unix_ms: 1,
                        event: ConversationActivityEvent::MessageRecorded { message_ref },
                    },
                    ConversationActivity {
                        activity_ref: "cwp://example.test/activities/two".into(),
                        sequence: 2,
                        occurred_at_unix_ms: 2,
                        event: ConversationActivityEvent::ExecutionActivity {
                            activity: crate::adapter::ExecutionActivity {
                                phase: crate::adapter::ExecutionPhase::UsingTool,
                                state: crate::adapter::ExecutionPhaseState::Updated,
                                completed_parts: Some(1),
                                total_parts: Some(2),
                            },
                        },
                    },
                    ConversationActivity {
                        activity_ref: "cwp://example.test/activities/three".into(),
                        sequence: 3,
                        occurred_at_unix_ms: 3,
                        event: ConversationActivityEvent::WorkUpdate {
                            invocation_ref: "cwp://example.test/invocations/one".into(),
                            source_message_ref: "cwp://example.test/messages/one".into(),
                            parent_invocation_ref: None,
                            wait: Some(Box::new(
                                crate::v2026_09::conversation::WorkInvocationWait {
                                    waiter_invocation_ref:
                                        "cwp://example.test/invocations/one".into(),
                                    awaited_invocation_ref:
                                        "cwp://example.test/invocations/two".into(),
                                    call_id: "call-1".into(),
                                    state: crate::v2026_09::invocation::InvocationWaitState::Pending,
                                    resume_after_activity_sequence: Some(3),
                                },
                            )),
                            actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
                            status: ActionResponseStatus::PROGRESS,
                            text: Some("working".into()),
                            text_kind: Some(
                                crate::v2026_09::conversation::WorkUpdateTextKind::Progress,
                            ),
                            activity: Some(Box::new(
                                crate::v2026_09::conversation::WorkActivity {
                                    activity_ref: "work-1".into(),
                                    kind: "command".into(),
                                    label: "Run checks".into(),
                                    state: "running".into(),
                                    started_at_unix_ms: 3,
                                    child_invocation_ref: None,
                                    duration_ms: None,
                                    command: Some("cargo test".into()),
                                    output: None,
                                    error: None,
                                    exit_code: None,
                                    files: vec![
                                        crate::v2026_09::conversation::WorkFileChange {
                                            path: "file.rs".into(),
                                            kind: "modified".into(),
                                            diff: None,
                                        },
                                    ],
                                },
                            )),
                        },
                    },
                    ConversationActivity {
                        activity_ref: "cwp://example.test/activities/four".into(),
                        sequence: 4,
                        occurred_at_unix_ms: 4,
                        event: ConversationActivityEvent::Progress {
                            progress: Some(crate::v2026_09::invocation::ActionProgress {
                                key: "tests".into(),
                                phase: Some("running".into()),
                                completed: Some(1),
                                total: Some(2),
                                unit: Some("suites".into()),
                            }),
                            status: ActionResponseStatus::PROGRESS,
                            summary: "Testing".into(),
                        },
                    },
                ],
                has_next_page: false,
                has_previous_page: false,
                total_count: 0,
                revision: "0".into(),
            },
            },
        },
    })
    .unwrap()
}

#[test]
fn all_retained_data_action_descriptions_keep_their_distinct_purposes() {
    let mut graph = crate::v2026_09::graph::resolve_graph(fixture_workspace()).unwrap();
    graph.permissions.read_retained_data = crate::v2026_09::graph::RetainedDataReadAccess::All;
    let graph = runtime_graph(graph, &fixture_workspace()).unwrap();
    for (name, purpose) in [
        ("list--conversations", "List root conversations"),
        ("read--conversation", "Read the timeline"),
        ("browse--conversation", "Read recent, related, searched, or excerpted messages"),
        ("debug--invocation", "Inspect an exact invocation"),
    ] {
        let route = graph.routes.iter().find(|route| route.actor_ref == INTRINSIC_CWR_ACTOR_REF && route.action_name == name).unwrap();
        assert!(route.action_description.starts_with(purpose));
        assert!(route.action_description.contains("readable by all callers in this runtime"));
        assert!(!route.action_description.contains("in which the caller participates"));
    }
    assert!(graph.routes.iter().find(|route| route.action_name == "browse--conversation").unwrap().action_description.contains("maximum is 2048 bytes"));
}
