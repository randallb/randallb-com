use super::*;

#[test]
fn summary_record_read_requires_a_participant_actor() {
    let command = HostCommand::ReadConversationRecords {
        version: HOST_COMMAND_VERSION,
        request_id: "read-summary".into(),
        source_ref: SOURCE_REF.into(),
        read: ConversationRecordReadRequest::Summary {
            conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
            workspace_owner_context_ref: None,
            participant_actor_ref: format!("{RUNTIME_REF}/actors/person"),
        },
        timeout_ms: 1_000,
    };
    command.validate().unwrap();

    let mut missing = serde_json::to_value(&command).unwrap();
    missing["read"]
        .as_object_mut()
        .unwrap()
        .remove("participant_actor_ref");
    assert!(serde_json::from_value::<HostCommand>(missing).is_err());

    let mut invalid = command;
    let HostCommand::ReadConversationRecords {
        read:
            ConversationRecordReadRequest::Summary {
                participant_actor_ref,
                ..
            },
        ..
    } = &mut invalid
    else {
        unreachable!()
    };
    *participant_actor_ref = "not-an-actor-ref".into();
    assert!(invalid.validate().is_err());
}

#[test]
fn root_conversation_page_rejects_malformed_summary_metadata() {
    let summary = |order, slug: &str| ConversationSummary {
        completed_work: vec![],
        key: ConversationSummaryKey {
            creation_order: order,
            conversation_ref: format!("{RUNTIME_REF}/conversations/{slug}"),
        },
        title: None,
        archived: false,
        last_message_ref: Some(format!("{RUNTIME_REF}/messages/{slug}")),
        last_message_preview: Some(slug.into()),
        completed_work_refs: Vec::new(),
        revision: "1".into(),
    };
    let event = |summaries: Vec<ConversationSummary>| {
        let total_count = summaries.len();
        HostEvent::RootConversationsPage {
            request_id: "read-roots".into(),
            source_ref: SOURCE_REF.into(),
            page: ConversationSummaryPage {
                membership_revision: "members-1".into(),
                revision: "1".into(),
                summaries,
                has_next_page: false,
                has_previous_page: false,
                total_count,
                archived_count: 0,
            },
        }
    };

    event(vec![summary(2, "two"), summary(1, "one")])
        .validate()
        .unwrap();
    assert!(event(vec![summary(1, "one"), summary(2, "two")])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("newest-first"));

    let mut missing_preview = summary(1, "one");
    missing_preview.last_message_preview = None;
    assert!(event(vec![missing_preview])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("appear together"));
}

#[test]
fn conversation_record_summary_validates_the_same_metadata_as_root_pages() {
    let event = |summary| HostEvent::ConversationRecordsPage {
        request_id: "read-summary".into(),
        source_ref: SOURCE_REF.into(),
        page: ConversationRecordPage::Summary { summary },
    };
    let summary = ConversationSummary {
        completed_work: vec![],
        key: ConversationSummaryKey {
            creation_order: 1,
            conversation_ref: format!("{RUNTIME_REF}/conversations/one"),
        },
        title: Some("First conversation".into()),
        archived: false,
        last_message_ref: Some(format!("{RUNTIME_REF}/messages/one")),
        last_message_preview: Some("Hello".into()),
        completed_work_refs: vec![],
        revision: "1".into(),
    };
    event(Some(summary.clone())).validate().unwrap();
    event(None).validate().unwrap();

    let mut malformed = summary;
    malformed.last_message_preview = None;
    assert!(event(Some(malformed))
        .validate()
        .unwrap_err()
        .to_string()
        .contains("appear together"));
}

#[test]
fn legacy_conversation_summary_defaults_completed_work_refs() {
    let summary: ConversationSummary = serde_json::from_value(json!({
        "key": {
            "creation_order": 0,
            "conversation_ref": format!("{RUNTIME_REF}/conversations/legacy")
        },
        "title": null,
        "archived": false,
        "last_message_ref": null,
        "last_message_preview": null,
        "revision": "1"
    }))
    .unwrap();
    assert!(summary.completed_work_refs.is_empty());
    let encoded = serde_json::to_value(summary).unwrap();
    assert!(encoded.get("completed_work_refs").is_none());
}
