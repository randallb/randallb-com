fn timeline_item_for<'a>(
    timeline: &'a ConversationTimeline,
    content_ref: &str,
) -> &'a ConversationTimelineItem {
    timeline
        .connections
        .iter()
        .flat_map(|connection| &connection.items)
        .find(|item| item.content.reference() == content_ref)
        .unwrap_or_else(|| panic!("missing timeline placement for {content_ref}"))
}

#[test]
fn managed_rollout_places_nested_work_atomically_and_reopens() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("managed-timeline.sqlite");
    let mut store = open(&path);
    store
        .configure_timeline_rollout(ConversationTimelineRollout::NewConversations)
        .unwrap();

    let ConversationAdmissionOutcome::Started(admitted) = store.commit(prepared_root()).unwrap()
    else {
        panic!("managed root admission must start");
    };
    let timeline = store.retained_timeline(CONVERSATION_REF).unwrap();
    assert_eq!(
        admitted.conversation.timeline_connection_ref.as_deref(),
        Some(timeline.root_connection_ref.as_str())
    );
    let root = timeline.connection(&timeline.root_connection_ref).unwrap();
    assert_eq!(root.items.len(), 2);
    assert!(matches!(
        root.items[0].content,
        ConversationTimelineContent::Message { ref message_ref }
            if message_ref == SOURCE_MESSAGE_REF
    ));
    assert!(matches!(
        root.items[1].content,
        ConversationTimelineContent::Invocation { ref invocation_ref }
            if invocation_ref == ROOT_INVOCATION_REF
    ));
    let root_item_ref = root.items[1].item_ref.clone();
    assert_eq!(
        store
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .source_message_ref
            .as_deref(),
        Some(SOURCE_MESSAGE_REF)
    );

    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let child_ref = "cwp://com.example.runtime/invocations/managed-child";
    let call = start_contract_child(&mut store, ROOT_INVOCATION_REF, child_ref, "managed-child");
    let timeline = store.retained_timeline(CONVERSATION_REF).unwrap();
    let root_item = timeline.item(&root_item_ref).unwrap();
    let root_children = timeline
        .connection(root_item.child_connection_ref.as_deref().unwrap())
        .unwrap();
    assert_eq!(root_children.items.len(), 2);
    assert_eq!(root_children.items[0].content.reference(), call.message_ref);
    assert_eq!(root_children.items[1].content.reference(), child_ref);

    store.claim_invocation(child_ref, 21).unwrap();
    store
        .record_invocation_progress_with_publication(child_ref, 102, "Nested progress", true)
        .unwrap();
    let progress_ref = store
        .conversation(CONVERSATION_REF)
        .unwrap()
        .messages
        .last()
        .unwrap()
        .message_ref
        .clone();
    let timeline = store.retained_timeline(CONVERSATION_REF).unwrap();
    let child_item = timeline_item_for(timeline, child_ref);
    let child_connection = timeline
        .connection(child_item.child_connection_ref.as_deref().unwrap())
        .unwrap();
    assert_eq!(child_connection.items.len(), 1);
    assert_eq!(child_connection.items[0].content.reference(), progress_ref);

    let PublishInvocationTerminalOutcome::Accepted {
        message: Some(child_reply),
        ..
    } = store
        .publish_child_action_terminal(
            child_ref,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Child done".into(),
                result: Some(json!({"artifact_ref": "cwp://com.example.runtime/artifacts/brief"})),
                error: None,
            },
            "Drafted the brief.".into(),
            30,
        )
        .unwrap()
    else {
        panic!("managed child completion must publish a reply");
    };
    let PublishInvocationTerminalOutcome::Accepted {
        message: Some(root_reply),
        ..
    } = store
        .publish_root_message_completed(
            ROOT_INVOCATION_REF,
            ActionResponseStatus::COMPLETED,
            json!({"text": "Root done"}),
            "Root done".into(),
            40,
        )
        .unwrap()
    else {
        panic!("managed root completion must publish a reply");
    };
    let timeline = store.retained_timeline(CONVERSATION_REF).unwrap();
    let child_item = timeline_item_for(timeline, child_ref);
    let child_connection = timeline
        .connection(child_item.child_connection_ref.as_deref().unwrap())
        .unwrap();
    assert_eq!(
        child_connection
            .items
            .iter()
            .map(|item| item.content.reference())
            .collect::<Vec<_>>(),
        [progress_ref.as_str(), child_reply.message_ref.as_str()]
    );
    let root_item = timeline_item_for(timeline, ROOT_INVOCATION_REF);
    let root_children = timeline
        .connection(root_item.child_connection_ref.as_deref().unwrap())
        .unwrap();
    assert_eq!(
        root_children.items.last().unwrap().content.reference(),
        root_reply.message_ref
    );
    let expected = timeline.clone();
    let export = store
        .export_conversation(
            CONVERSATION_REF,
            "cwp://com.example.fixture/actors/requester",
        )
        .unwrap();
    let exported = export.timeline.as_ref().unwrap();
    assert_eq!(exported.work.len(), 2);
    assert!(exported.work.iter().all(|work| work.status.is_completed()));
    let child = exported
        .work
        .iter()
        .find(|work| work.invocation_ref == child_ref)
        .unwrap();
    assert_eq!(
        child.parent_invocation_ref.as_deref(),
        Some(ROOT_INVOCATION_REF)
    );
    assert_eq!(
        child.reply_message_ref.as_deref(),
        Some(child_reply.message_ref.as_str())
    );
    assert_eq!(exported.timeline, expected);
    drop(store);

    let reopened = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        RUNTIME_ROOT,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    assert_eq!(
        reopened.retained_timeline(CONVERSATION_REF),
        Some(&expected)
    );
}

#[test]
fn disabled_runtime_rollout_keeps_activated_history_writable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("disabled-timeline.sqlite");
    let mut store = open(&path);
    store
        .configure_timeline_rollout(ConversationTimelineRollout::NewConversations)
        .unwrap();
    store.commit(prepared_root()).unwrap();
    drop(store);

    let mut reopened = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        RUNTIME_ROOT,
        ConversationTimelineRollout::Disabled,
    )
    .unwrap();
    let before = reopened
        .retained_timeline(CONVERSATION_REF)
        .unwrap()
        .clone();
    reopened.claim_invocation(ROOT_INVOCATION_REF, 100).unwrap();
    assert_eq!(reopened.retained_timeline(CONVERSATION_REF), Some(&before));
}

#[test]
fn unconfigured_writers_reject_an_activated_database_before_recovery() {
    for bound in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!(
            "unconfigured-{}-timeline.sqlite",
            if bound { "bound" } else { "plain" }
        ));
        let mut store = if bound {
            ConversationAdmissionStore::open_bound(
                &path,
                RUNTIME_ROOT,
                "workspace-a",
                directory.path(),
            )
            .unwrap()
        } else {
            ConversationAdmissionStore::open(&path, RUNTIME_ROOT).unwrap()
        };
        store
            .configure_timeline_rollout(ConversationTimelineRollout::NewConversations)
            .unwrap();
        store.commit(prepared_root()).unwrap();
        let revision_before = store.commit_revision;
        drop(store);

        let reopened = if bound {
            ConversationAdmissionStore::open_bound(
                &path,
                RUNTIME_ROOT,
                "workspace-a",
                directory.path(),
            )
        } else {
            ConversationAdmissionStore::open(&path, RUNTIME_ROOT)
        };
        let error = match reopened {
            Ok(_) => panic!("activated state requires a configured writer"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("requires an explicitly configured writer")
        );
        assert_eq!(
            cwpdb_store::test_support::invocation_reads::commit_revision(&path, RUNTIME_ROOT)
                .unwrap(),
            revision_before
        );
    }
}

#[test]
fn startup_checks_rollout_before_reconciliation_and_places_the_repair() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("guarded-reconciliation.sqlite");
    let mut store = open(&path);
    store
        .configure_timeline_rollout(ConversationTimelineRollout::NewConversations)
        .unwrap();
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
        .finish_invocation(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Answered".into(),
                result: Some(json!({"text": "Reconciled canonical answer."})),
                error: None,
            },
            30,
        )
        .unwrap();
    assert!(
        store
            .admission_record_for_invocation(ROOT_INVOCATION_REF)
            .unwrap()
            .reply_message_ref
            .is_none()
    );
    drop(store);
    let reopened = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        RUNTIME_ROOT,
        ConversationTimelineRollout::Disabled,
    )
    .unwrap();
    let reply_ref = reopened
        .admission_record_for_invocation(ROOT_INVOCATION_REF)
        .unwrap()
        .reply_message_ref
        .as_deref()
        .expect("startup reconciles the terminal reply");
    let timeline = reopened.retained_timeline(CONVERSATION_REF).unwrap();
    assert!(
        timeline
            .connections
            .iter()
            .flat_map(|connection| &connection.items)
            .any(|item| item.content.reference() == reply_ref)
    );
}

#[test]
fn managed_rollout_preserves_an_undisclosed_parent_for_owned_output() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("undisclosed-managed-output.sqlite");
    let mut store = open(&path);
    let ConversationAdmissionOutcome::Started(_) = store.commit(prepared_root()).unwrap() else {
        panic!("manual root admission must start");
    };
    let timeline_ref = format!("{CONVERSATION_REF}/manual-timeline");
    store
        .create_retained_timeline(CONVERSATION_REF, &timeline_ref)
        .unwrap();
    store
        .publish_timeline_message(
            CONVERSATION_REF,
            &format!("{CONVERSATION_REF}/publications/source"),
            SOURCE_MESSAGE_REF,
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    store
        .retain_invocation_timeline_publication(
            ROOT_INVOCATION_REF,
            SOURCE_MESSAGE_REF,
            &format!("{CONVERSATION_REF}/publications/undisclosed-root"),
            ConversationTimelineDisposition::Undisclosed,
        )
        .unwrap();
    store
        .configure_timeline_rollout(ConversationTimelineRollout::NewConversations)
        .unwrap();

    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
        .record_invocation_progress_with_publication(
            ROOT_INVOCATION_REF,
            102,
            "Private progress",
            true,
        )
        .unwrap();
    let progress_ref = store
        .conversation(CONVERSATION_REF)
        .unwrap()
        .messages
        .last()
        .unwrap()
        .message_ref
        .clone();
    let PublishInvocationTerminalOutcome::Accepted {
        message: Some(reply),
        ..
    } = store
        .publish_root_message_completed(
            ROOT_INVOCATION_REF,
            ActionResponseStatus::COMPLETED,
            json!({"text": "Private answer"}),
            "Private answer".into(),
            30,
        )
        .unwrap()
    else {
        panic!("undisclosed managed completion must still commit");
    };

    let timeline = store.retained_timeline(CONVERSATION_REF).unwrap();
    let visible_refs = timeline
        .connections
        .iter()
        .flat_map(|connection| &connection.items)
        .map(|item| item.content.reference())
        .collect::<Vec<_>>();
    assert_eq!(visible_refs, [SOURCE_MESSAGE_REF]);
    assert!(!visible_refs.contains(&progress_ref.as_str()));
    assert!(!visible_refs.contains(&reply.message_ref.as_str()));
    assert!(
        store
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .timeline_publications
            .values()
            .all(|disposition| *disposition == ConversationTimelineDisposition::Undisclosed)
    );
}

#[path = "timeline_rollout/forks.rs"]
mod rollout_forks;
#[path = "timeline_rollout/guards.rs"]
mod rollout_guards;
