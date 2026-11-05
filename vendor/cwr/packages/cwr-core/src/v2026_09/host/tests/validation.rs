#[test]
fn validation_rejects_children_and_private_activity() {
    let mut child = conversation();
    child.parent_conversation_ref = Some(format!("{RUNTIME_REF}/conversations/parent"));
    let child_event = HostEvent::ConversationUpsert {
        source_ref: SOURCE_REF.into(),
        conversation: child,
    };
    assert!(child_event
        .validate()
        .expect_err("reject child conversation")
        .to_string()
        .contains("must contain roots"));

    let mut private = conversation();
    private.activity.push(ConversationActivity {
        activity_ref: format!("{RUNTIME_REF}/activities/private"),
        sequence: 1,
        occurred_at_unix_ms: 1,
        event: ConversationActivityEvent::ProviderThread {
            provider: "slack".into(),
            thread_id: "private-thread".into(),
        },
    });
    let private_event = HostEvent::ConversationUpsert {
        source_ref: SOURCE_REF.into(),
        conversation: private,
    };
    assert!(private_event
        .validate()
        .expect_err("reject provider-private activity")
        .to_string()
        .contains("provider-private"));
}

#[test]
fn validation_rejects_unnormalized_conversation_titles() {
    for title in [" padded ", "line\nbreak", "line\u{2028}break"] {
        let mut retained = conversation();
        retained.title = Some(title.into());
        let event = HostEvent::ConversationUpsert {
            source_ref: SOURCE_REF.into(),
            conversation: retained,
        };
        assert!(
            event.validate().is_err(),
            "accepted invalid title {title:?}"
        );
    }
}

#[test]
fn ready_validation_rejects_unknown_fields_and_duplicate_sources() {
    let unknown = json!({
        "kind": "ready",
        "sources": [source()],
        "token": "private"
    });
    assert!(serde_json::from_value::<HostEvent>(unknown).is_err());

    let event = HostEvent::Ready {
        sources: vec![source(), source()],
    };
    assert!(event
        .validate()
        .expect_err("reject duplicate sources")
        .to_string()
        .contains("repeats conversation source"));
}
