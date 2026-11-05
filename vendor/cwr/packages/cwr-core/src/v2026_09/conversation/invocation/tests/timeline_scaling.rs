#[test]
fn timeline_progress_updates_only_its_invocation_with_growing_retained_work() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = open(&directory.path().join("history.sqlite"));
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    audit_timeline_writes(&store);
    let mut last_child_call = None;
    for count in 0..=32 {
        if count > 0 {
            let child = format!("{RUNTIME_ROOT}/invocations/history-{count}");
            last_child_call = Some(start_contract_child(
                &mut store,
                ROOT_INVOCATION_REF,
                &child,
                &format!("history-{count}"),
            ));
            store.claim_invocation(&child, 21).unwrap();
            store
                .publish_child_action_terminal(
                    &child,
                    InvocationTerminal {
                        status: ActionResponseStatus::COMPLETED,
                        summary: "Retained result".into(),
                        result: Some(json!({"done":true})),
                        error: None,
                    },
                    "Retained result".into(),
                    30,
                )
                .unwrap();
        }
        publication_write_audit(&store, cwpdb_store::conversation::PublicationAudit::Drain);
        let before = timeline_page(
            &store,
            ConversationRecordWindow {
                last: Some(1),
                ..Default::default()
            },
        );
        store
            .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 102, "Another update")
            .unwrap();
        assert_activity_only_writes(&store, 1);
        let after = timeline_page(
            &store,
            ConversationRecordWindow {
                last: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(before.revision, after.revision);
    }
    let retained_activities = cwpdb_store::test_support::invocation_reads::retained_activity_count(
        &directory.path().join("history.sqlite"),
        CONVERSATION_REF,
    )
    .unwrap();
    assert!(retained_activities > 1);
    publication_write_audit(&store, cwpdb_store::conversation::PublicationAudit::Drain);
    store
        .append_action_result(
            CONVERSATION_REF,
            "cwp://com.example.fixture/actors/worker",
            "cwp://com.example.fixture/actors/requester",
            "@worker--draft--brief",
            json!({}),
            ConversationActionOutcome::Succeeded,
            "Visible reply after retained work".into(),
            &last_child_call.unwrap().message_ref,
        )
        .unwrap();
    assert!(
        publication_write_audit(&store, cwpdb_store::conversation::PublicationAudit::Drain)
            .activity
            <= 1,
        "adding a visible reply must not rerun the activity writer for {retained_activities} retained activities"
    );
    capture_timeline_contract(&store, "retained_history");
}

#[test]
fn timeline_projection_accepts_legacy_work_update_attachment() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy-timeline.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
        .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 102, "Legacy progress")
        .unwrap();
    drop(store);

    let updated =
        cwpdb_store::test_support::invocation_reads::legacy_work_attachments(&path).unwrap();
    assert!(updated > 0, "fixture must contain a stored work update");

    let mut reopened = open(&path);
    reopened
        .append_action_result(
            CONVERSATION_REF,
            "cwp://com.example.fixture/actors/worker",
            "cwp://com.example.fixture/actors/requester",
            "@worker--draft--brief",
            json!({}),
            ConversationActionOutcome::Succeeded,
            "Refresh legacy timeline".into(),
            SOURCE_MESSAGE_REF,
        )
        .unwrap();
    let legacy_attachments =
        cwpdb_store::test_support::invocation_reads::legacy_attachment_count(&path).unwrap();
    assert_eq!(
        legacy_attachments, 0,
        "projection rewrites legacy activity payloads"
    );
    assert!(timeline_page(
        &reopened,
        ConversationRecordWindow {
            first: Some(10),
            ..Default::default()
        },
    )
    .activity
    .iter()
    .any(|activity| matches!(
        &activity.event,
        ConversationActivityEvent::WorkUpdate { text: Some(text), .. } if text == "Legacy progress"
    )));
}
