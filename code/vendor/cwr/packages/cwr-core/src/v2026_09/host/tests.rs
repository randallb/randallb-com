use serde_json::json;

use super::*;
use crate::v2026_09::conversation::{
    ConversationActivity, ConversationContent, ConversationContentPage, ConversationMessage,
    ConversationPositionKey, ConversationRecordPageRequest, ConversationSummary,
    ConversationSummaryKey, ConversationTimelinePage,
};

const RUNTIME_REF: &str = "cwp://com.example.runtime/slack";
const SOURCE_REF: &str = "cwp://com.example.runtime/slack/hosts/slack";

fn source() -> ConversationSource {
    ConversationSource {
        source_ref: SOURCE_REF.into(),
        runtime_ref: RUNTIME_REF.into(),
        display_name: "Slack · Example".into(),
        capabilities: ConversationSourceCapabilities::read_only(),
        performance_database_path: None,
        build_identity: None,
    }
}

#[test]
fn capability_presets_do_not_claim_timeline_reads() {
    assert!(!ConversationSourceCapabilities::read_only().can_read_timeline);
    assert!(!ConversationSourceCapabilities::continuable().can_read_timeline);
    assert!(!ConversationSourceCapabilities::controllable().can_read_timeline);
}

#[test]
fn false_fork_capability_is_omitted_for_older_strict_readers() {
    let mut capabilities = ConversationSourceCapabilities::read_only();
    let legacy_shape = serde_json::to_value(capabilities).unwrap();
    assert!(legacy_shape.get("can_fork").is_none());

    capabilities.can_fork = true;
    assert_eq!(
        serde_json::to_value(capabilities).unwrap()["can_fork"],
        true
    );
}

#[test]
fn performance_database_paths_are_optional_and_local() {
    let ready = HostEvent::Ready {
        sources: vec![source()],
    };
    let value = serde_json::to_value(&ready).unwrap();
    assert!(value["sources"][0]
        .get("performance_database_path")
        .is_none());

    let mut invalid_source = source();
    invalid_source.performance_database_path = Some("metrics.sqlite3".into());
    assert!(HostEvent::Ready {
        sources: vec![invalid_source],
    }
    .validate()
    .is_err());

    let mut local_source = source();
    local_source.performance_database_path = Some("/tmp/metrics.sqlite3".into());
    HostEvent::Ready {
        sources: vec![local_source],
    }
    .validate()
    .unwrap();
}

fn conversation() -> RetainedConversation {
    RetainedConversation {
        owner_context_ref: None,
        conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
        title: None,
        archived: false,
        sidebar_eligible: None,
        parent_conversation_ref: None,
        participant_actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("person").into()],
        messages: vec![ConversationMessage {
            file_refs: Vec::new(),
            progress: None,
            message_ref: format!("{RUNTIME_REF}/messages/one"),
            author_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
            addressed_actor_refs: Vec::new(),
            addressing_mode: Default::default(),
            text: "Hello".into(),
            visualizations: Vec::new(),
            reply_to_message_ref: None,
            action: None,
        }],
        timeline_connection_ref: None,
        activity: Vec::new(),
    }
}

include!("workspace_tests.rs");
include!("tests/validation.rs");

#[test]
fn events_round_trip_with_stable_tags() {
    let events = [
        HostEvent::Ready {
            sources: vec![source()],
        },
        HostEvent::ConversationSnapshot {
            source_ref: SOURCE_REF.into(),
            conversations: vec![conversation()],
        },
        HostEvent::RootConversationsPage {
            request_id: "read-roots".into(),
            source_ref: SOURCE_REF.into(),
            page: ConversationSummaryPage {
                membership_revision: "members-1".into(),
                revision: "1".into(),
                summaries: vec![ConversationSummary {
                    completed_work: vec![],
                    key: ConversationSummaryKey {
                        creation_order: 1,
                        conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
                    },
                    title: Some("First conversation".into()),
                    archived: false,
                    last_message_ref: Some(format!("{RUNTIME_REF}/messages/one")),
                    last_message_preview: Some("Hello".into()),
                    completed_work_refs: Vec::new(),
                    revision: "canonical:v1:summary".into(),
                }],
                has_next_page: false,
                has_previous_page: false,
                total_count: 1,
                archived_count: 0,
            },
        },
        HostEvent::ConversationRecordsPage {
            request_id: "read-content".into(),
            source_ref: SOURCE_REF.into(),
            page: ConversationRecordPage::Content {
                page: ConversationContentPage {
                    content: vec![ConversationContent {
                        key: ConversationPositionKey {
                            position: 1,
                            revision: None,
                        },
                        message: conversation().messages[0].clone(),
                        occurred_at_unix_ms: None,
                    }],
                    has_next_page: false,
                    has_previous_page: false,
                    total_count: 1,
                    revision: "canonical:v1:content".into(),
                },
            },
        },
        HostEvent::ConversationRecordsPage {
            request_id: "read-timeline".into(),
            source_ref: SOURCE_REF.into(),
            page: ConversationRecordPage::Timeline {
                page: ConversationTimelinePage {
                    anchors: Vec::new(),
                    messages: Vec::new(),
                    activity: Vec::new(),
                    has_next_page: false,
                    has_previous_page: false,
                    total_count: 0,
                    revision: "canonical:v1:timeline".into(),
                },
            },
        },
        HostEvent::ConversationUpsert {
            source_ref: SOURCE_REF.into(),
            conversation: conversation(),
        },
        HostEvent::ConversationLiveOutput {
            source_ref: SOURCE_REF.into(),
            conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
            output: Some(ConversationLiveOutput {
                invocation_ref: format!("{RUNTIME_REF}/invocations/one"),
                generation: 1,
                target_actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
                phase: AssistantPhase::FinalAnswer,
                text: "Hello from the model".into(),
            }),
        },
        HostEvent::ConversationLiveStatusSnapshot {
            source_ref: SOURCE_REF.into(),
            conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
            statuses: vec![ConversationLiveStatus {
                subject_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
                publisher_session_id: "desktop-session".into(),
                sequence: 1,
                target_actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                status: ConversationLiveStatusName::Composing,
                expires_at_unix_ms: 7_000,
            }],
        },
        HostEvent::ConversationTurnStatusSnapshot {
            source_ref: SOURCE_REF.into(),
            conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
            turns: vec![ConversationTurnStatus {
                questions: vec![],
                request_preview: None,
                invocation_ref: format!("{RUNTIME_REF}/invocations/one"),
                source_message_ref: format!("{RUNTIME_REF}/messages/one"),
                requester_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
                target_actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
                state: ConversationTurnState::Pending,
                admitted_at_unix_ms: 1,
                started_at_unix_ms: None,
                workers: Vec::new(),
            }],
        },
        HostEvent::ConversationRemoved {
            source_ref: SOURCE_REF.into(),
            conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
        },
    ];
    let expected_kinds = [
        "ready",
        "conversation_snapshot",
        "root_conversations_page",
        "conversation_records_page",
        "conversation_records_page",
        "conversation_upsert",
        "conversation_live_output",
        "conversation_live_status_snapshot",
        "conversation_turn_status_snapshot",
        "conversation_removed",
    ];
    for (event, expected_kind) in events.iter().zip(expected_kinds) {
        event.validate().expect("validate host event");
        let value = serde_json::to_value(event).expect("serialize host event");
        assert_eq!(value["kind"], expected_kind);
        let decoded: HostEvent = serde_json::from_value(value).expect("deserialize host event");
        assert_eq!(decoded, *event);
    }
}

#[test]
fn root_conversation_page_command_is_strict_and_bounded() {
    let command = HostCommand::ReadRootConversations {
        version: HOST_COMMAND_VERSION,
        request_id: "read-roots".into(),
        source_ref: SOURCE_REF.into(),
        page: ConversationSummaryPageRequest {
            first: Some(50),
            ..Default::default()
        },
        timeout_ms: 1_000,
    };
    command.validate().unwrap();
    let mut completion_aware = command.clone();
    if let HostCommand::ReadRootConversations { version, .. } = &mut completion_aware {
        *version = 2;
    }
    completion_aware.validate().unwrap();
    if let HostCommand::ReadRootConversations { version, .. } = &mut completion_aware {
        *version = 3;
    }
    completion_aware.validate().unwrap();
    if let HostCommand::ReadRootConversations { version, .. } = &mut completion_aware {
        *version = 4;
    }
    assert!(completion_aware.validate().is_err());
    let value = serde_json::to_value(&command).unwrap();
    assert_eq!(value["kind"], "read_root_conversations");
    assert_eq!(
        serde_json::from_value::<HostCommand>(value).unwrap(),
        command
    );

    let mut invalid = command.clone();
    let HostCommand::ReadRootConversations { timeout_ms, .. } = &mut invalid else {
        unreachable!()
    };
    *timeout_ms = 0;
    assert!(invalid
        .validate()
        .unwrap_err()
        .to_string()
        .contains("timeout"));
}

#[test]
fn conversation_record_page_command_is_strict_and_bounded() {
    let command = HostCommand::ReadConversationRecords {
        version: HOST_COMMAND_VERSION,
        request_id: "read-content".into(),
        source_ref: SOURCE_REF.into(),
        read: ConversationRecordReadRequest::Content {
            conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
            page: ConversationRecordPageRequest {
                last: Some(MAX_CONVERSATION_RECORD_PAGE_SIZE),
                ..Default::default()
            },
        },
        timeout_ms: 1_000,
    };
    command.validate().unwrap();
    let value = serde_json::to_value(&command).unwrap();
    assert_eq!(value["kind"], "read_conversation_records");
    assert_eq!(
        serde_json::from_value::<HostCommand>(value).unwrap(),
        command
    );

    let mut invalid = command;
    let HostCommand::ReadConversationRecords { read, .. } = &mut invalid else {
        unreachable!()
    };
    let ConversationRecordReadRequest::Content { page, .. } = read else {
        panic!("expected content read request");
    };
    page.last = Some(MAX_CONVERSATION_RECORD_PAGE_SIZE + 1);
    assert!(invalid
        .validate()
        .unwrap_err()
        .to_string()
        .contains("page size"));
}

mod summary;

#[test]
fn turn_status_validates_worker_identities() {
    let valid = json!({
        "kind": "conversation_turn_status_snapshot",
        "source_ref": SOURCE_REF,
        "conversation_ref": format!("{RUNTIME_REF}/conversations/one"),
        "turns": [{
            "invocation_ref": format!("{RUNTIME_REF}/invocations/one"),
            "source_message_ref": format!("{RUNTIME_REF}/messages/one"),
            "requester_actor_ref": crate::v2026_09::test_fixture::actor_ref("person"),
            "target_actor_ref": crate::v2026_09::test_fixture::actor_ref("helper"),
            "state": "running", "admitted_at_unix_ms": 1,
            "workers": [{
                "invocation_ref": format!("{RUNTIME_REF}/invocations/child"),
                "target_actor_ref": crate::v2026_09::test_fixture::actor_ref("helper"),
                "state": "running", "phase": "working", "admitted_at_unix_ms": 2
            }]
        }]
    });
    let event: HostEvent = serde_json::from_value(valid.clone()).unwrap();
    event.validate().unwrap();
    assert_eq!(
        serde_json::from_str::<HostEvent>(&serde_json::to_string(&event).unwrap()).unwrap(),
        event
    );
    for (field, label) in [
        ("invocation_ref", "worker invocation"),
        ("target_actor_ref", "worker target actor"),
    ] {
        let mut invalid = valid.clone();
        invalid["turns"][0]["workers"][0][field] = json!("not a CWR ref");
        let event: HostEvent = serde_json::from_value(invalid).unwrap();
        assert!(event.validate().unwrap_err().to_string().contains(label));
    }
    let mut invalid = valid;
    invalid["turns"][0]["workers"][0]["delegation_path_actor_refs"] = json!(["not a CWR ref"]);
    let event: HostEvent = serde_json::from_value(invalid).unwrap();
    assert!(event
        .validate()
        .unwrap_err()
        .to_string()
        .contains("worker delegation path actor"));
}

include!("tests/command.rs");
include!("create_tests.rs");

#[test]
fn live_status_command_is_leased_and_round_trips() {
    let command = HostCommand::SetConversationLiveStatus {
        version: HOST_COMMAND_VERSION,
        request_id: "desktop-3".into(),
        source_ref: SOURCE_REF.into(),
        conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
        subject_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
        publisher_session_id: "desktop-session".into(),
        sequence: 1,
        target_actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
        status: Some(ConversationLiveStatusName::Composing),
        lease_ms: 7_000,
    };
    command.validate().expect("validate live status command");
    let wire = serde_json::to_value(&command).expect("serialize live status command");
    assert_eq!(wire["kind"], "set_conversation_live_status");
    assert_eq!(
        serde_json::from_value::<HostCommand>(wire).expect("decode live status command"),
        command
    );

    let invalid_clear = HostCommand::SetConversationLiveStatus {
        version: HOST_COMMAND_VERSION,
        request_id: "desktop-4".into(),
        source_ref: SOURCE_REF.into(),
        conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
        subject_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
        publisher_session_id: "desktop-session".into(),
        sequence: 2,
        target_actor_refs: Vec::new(),
        status: None,
        lease_ms: 7_000,
    };
    assert!(invalid_clear.validate().is_err());
}

#[test]
fn projection_removes_provider_private_activity() {
    let mut retained = conversation();
    retained.activity = vec![
        ConversationActivity {
            activity_ref: format!("{RUNTIME_REF}/activities/thread"),
            sequence: 1,
            occurred_at_unix_ms: 1,
            event: ConversationActivityEvent::ProviderThread {
                provider: "slack".into(),
                thread_id: "private-thread".into(),
            },
        },
        ConversationActivity {
            activity_ref: format!("{RUNTIME_REF}/activities/event"),
            sequence: 2,
            occurred_at_unix_ms: 2,
            event: ConversationActivityEvent::ProviderEvent {
                provider: "slack".into(),
                method: "raw_event".into(),
                data: json!({"token": "private"}),
            },
        },
        ConversationActivity {
            activity_ref: format!("{RUNTIME_REF}/activities/start"),
            sequence: 3,
            occurred_at_unix_ms: 3,
            event: ConversationActivityEvent::TurnStarted {
                invocation_ref: format!("{RUNTIME_REF}/invocations/one"),
                source_message_ref: format!("{RUNTIME_REF}/messages/one"),
                invocation: Some(json!({"integration_params": {"token": "private"}})),
            },
        },
    ];

    let projected = project_conversation(&retained);

    assert_eq!(projected.conversation_ref, retained.conversation_ref);
    assert_eq!(projected.messages, retained.messages);
    assert_eq!(projected.activity.len(), 1);
    assert!(matches!(
        &projected.activity[0].event,
        ConversationActivityEvent::TurnStarted {
            invocation: None,
            ..
        }
    ));
    let event = HostEvent::ConversationSnapshot {
        source_ref: SOURCE_REF.into(),
        conversations: vec![projected],
    };
    event.validate().expect("validate projected snapshot");
    let wire = serde_json::to_string(&event).expect("serialize projected snapshot");
    assert!(!wire.contains("private"));
}

#[test]
fn workspace_action_can_omit_source_but_conversation_action_cannot() {
    let wire = json!({
        "kind": "invoke_action", "version": HOST_COMMAND_VERSION,
        "request_id": "workspace-mail", "author_actor_ref": "~@/actors/person",
        "activation": "@workloop_ops--disconnect--mail", "input": {}
    });
    let action: HostCommand = serde_json::from_value(wire.clone()).unwrap();
    action.validate().unwrap();
    assert!(serde_json::to_value(&action)
        .unwrap()
        .get("source_ref")
        .is_none());
    for extra in [
        json!({"source_ref": ""}),
        json!({"conversation_ref": format!("{RUNTIME_REF}/conversations/one")}),
    ] {
        let mut invalid = wire.clone();
        invalid
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let invalid: HostCommand = serde_json::from_value(invalid).unwrap();
        assert!(invalid.validate().is_err());
    }
}
