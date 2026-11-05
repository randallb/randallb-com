fn start_legacy_migration_child(
    store: &mut ConversationAdmissionStore,
    claimed: &ClaimedConversationInvocation,
    conversation: &str,
    parent: &str,
    name: &str,
) -> String {
    use crate::v2026_09::invocation::{InvocationOrigin, StartActionInvocation};
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let call = store
        .append_action_call(
            conversation,
            helper,
            helper,
            "@helper--review--work",
            json!({}),
            "Review".into(),
        )
        .unwrap();
    let child = format!("cwp://com.example.runtime/invocations/{name}");
    let mut adapter = crate::adapter::AdapterInvocation::new(
        &child,
        "review--work",
        json!({}),
        json!({"type":"object"}),
    );
    adapter.integration_ref = claimed.adapter_invocation.integration_ref.clone();
    store
        .start_child_invocation(
            StartActionInvocation {
                invocation_ref: child.clone(),
                origin: InvocationOrigin::HostCall {
                    caller_invocation_ref: parent.to_owned(),
                    call_id: "review".into(),
                },
                requester_actor_ref: helper.into(),
                target_actor_ref: helper.into(),
                conversation_ref: Some(conversation.to_owned()),
                actor_integration_ref: claimed.claim.invocation.actor_integration_ref.clone(),
                action_name: "review--work".into(),
                input: json!({}),
                parent_invocation_ref: Some(parent.to_owned()),
                admitted_at_unix_ms: 200,
            },
            &call.message_ref,
            adapter,
        )
        .unwrap();
    store.claim_invocation(&child, 300).unwrap();
    child
}

#[test]
fn legacy_progress_is_preserved_under_its_invocation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("progress-legacy-timeline.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "progress-migration");
    let claimed = claim_working(&mut store, &turn);
    let conversation_ref = turn.record.conversation_ref.clone();
    let invocation_ref = turn.record.invocation_ref.clone().unwrap();
    store
        .record_commentary_progress(
            &invocation_ref,
            claimed.claim.invocation.attempt_count,
            AdapterCompletedCommentary {
                model_run_ref: claimed.model_run_ref.clone(),
                provider: "codex".into(),
                thread_id: Some("migration-thread".into()),
                response_or_turn_id: "migration-turn".into(),
                item_id: "migration-item".into(),
                text: "Published commentary".into(),
            },
        )
        .unwrap();
    settle_working(&mut store, &claimed);

    let before = store
        .conversation(&conversation_ref)
        .unwrap()
        .messages
        .clone();
    let progress = before
        .iter()
        .find(|message| message.progress.is_some())
        .unwrap();
    assert!(matches!(
        store
            .migrate_legacy_conversation_timeline(&conversation_ref)
            .unwrap(),
        ConversationTimelineMigrationOutcome::Migrated { .. }
    ));
    assert_eq!(
        store.conversation(&conversation_ref).unwrap().messages,
        before
    );
    let timeline = store.retained_timeline(&conversation_ref).unwrap();
    let root_items = &timeline
        .connection(&timeline.root_connection_ref)
        .unwrap()
        .items;
    let turn_item = root_items.iter().find(|item| matches!(&item.content,
        ConversationTimelineContent::Invocation { invocation_ref: reference } if reference == &invocation_ref)).unwrap();
    let child_ref = turn_item.child_connection_ref.clone().unwrap();
    let child = timeline.connection(&child_ref).unwrap().clone();
    assert!(child
        .items
        .iter()
        .any(|item| item.content.reference() == progress.message_ref));
    let mut reader = store.read_store().unwrap();
    let page = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap()
        .conversation_timeline_connection(
            &conversation_ref,
            &child_ref,
            &ConversationTimelinePageRequest {
                first: Some(100),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(page
        .edges
        .iter()
        .any(|edge| edge.item.content.reference() == progress.message_ref));
    drop(reader);
    drop(store);
    let reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    assert_eq!(
        reopened.conversation(&conversation_ref).unwrap().messages,
        before
    );
    assert_eq!(
        reopened
            .retained_timeline(&conversation_ref)
            .unwrap()
            .connection(&child_ref)
            .unwrap()
            .items,
        child.items
    );
}

#[test]
fn legacy_nested_work_and_progress_keep_their_original_sibling_order() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("nested.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let turn = working_turn(&mut store, None, "nested-migration");
    let claimed = claim_working(&mut store, &turn);
    let conversation = turn.record.conversation_ref;
    let parent = turn.record.invocation_ref.unwrap();
    let progress = |store: &mut ConversationAdmissionStore, id: &str| {
        store
            .record_commentary_progress(
                &parent,
                claimed.claim.invocation.attempt_count,
                AdapterCompletedCommentary {
                    model_run_ref: claimed.model_run_ref.clone(),
                    provider: "codex".into(),
                    thread_id: Some("nested-migration".into()),
                    response_or_turn_id: "turn".into(),
                    item_id: id.into(),
                    text: id.into(),
                },
            )
            .unwrap();
    };
    progress(&mut store, "before");
    let child = start_legacy_migration_child(&mut store, &claimed, &conversation, &parent, "child");
    let grandchild =
        start_legacy_migration_child(&mut store, &claimed, &conversation, &child, "grandchild");
    store
        .finish_invocation(
            &grandchild,
            crate::v2026_09::invocation::InvocationTerminal {
                status: crate::v2026_09::invocation::ActionResponseStatus::COMPLETED,
                summary: "Grandchild reviewed".into(),
                result: Some(json!({"text":"Grandchild reviewed"})),
                error: None,
            },
            350,
        )
        .unwrap();
    store
        .finish_invocation(
            &child,
            crate::v2026_09::invocation::InvocationTerminal {
                status: crate::v2026_09::invocation::ActionResponseStatus::COMPLETED,
                summary: "Reviewed".into(),
                result: Some(json!({"text":"Reviewed"})),
                error: None,
            },
            400,
        )
        .unwrap();
    progress(&mut store, "after");
    settle_working(&mut store, &claimed);
    let messages = store.conversation(&conversation).unwrap().messages.clone();
    assert!(matches!(
        store
            .migrate_legacy_conversation_timeline(&conversation)
            .unwrap(),
        ConversationTimelineMigrationOutcome::Migrated { .. }
    ));
    assert_eq!(
        store.conversation(&conversation).unwrap().messages,
        messages
    );
    let timeline = store.retained_timeline(&conversation).unwrap();
    let parent_item = timeline
        .connections
        .iter()
        .flat_map(|connection| &connection.items)
        .find(|item| item.content.reference() == parent)
        .unwrap();
    let children = &timeline
        .connection(parent_item.child_connection_ref.as_ref().unwrap())
        .unwrap()
        .items;
    let before = messages
        .iter()
        .find(|message| message.text == "before")
        .unwrap();
    let after = messages
        .iter()
        .find(|message| message.text == "after")
        .unwrap();
    assert_eq!(
        children
            .iter()
            .map(|item| item.content.reference())
            .collect::<Vec<_>>(),
        [
            before.message_ref.as_str(),
            child.as_str(),
            after.message_ref.as_str()
        ]
    );
    let child_item = children
        .iter()
        .find(|item| item.content.reference() == child)
        .unwrap();
    let grandchildren = &timeline
        .connection(child_item.child_connection_ref.as_ref().unwrap())
        .unwrap()
        .items;
    assert!(grandchildren
        .iter()
        .any(|item| item.content.reference() == grandchild));
    assert!(!timeline
        .connection(&timeline.root_connection_ref)
        .unwrap()
        .items
        .iter()
        .any(|item| item.content.reference() == grandchild));
    assert!(store.invocation_record(&grandchild).unwrap().timeline_publications.values().any(|disposition| matches!(disposition,
        ConversationTimelineDisposition::Child { parent_item_ref } if parent_item_ref == &child_item.item_ref)));
    assert_eq!(
        timeline
            .connection(&timeline.root_connection_ref)
            .unwrap()
            .items
            .iter()
            .filter_map(|item| {
                match &item.content {
                    ConversationTimelineContent::Message { message_ref } => {
                        Some(message_ref.as_str())
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>(),
        messages
            .iter()
            .filter(|message| message.progress.is_none())
            .map(|message| message.message_ref.as_str())
            .collect::<Vec<_>>()
    );
    let mut reader = store.read_store().unwrap();
    let page = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap()
        .conversation_timeline_connection(
            &conversation,
            child_item.child_connection_ref.as_ref().unwrap(),
            &ConversationTimelinePageRequest {
                first: Some(100),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(page
        .edges
        .iter()
        .any(|edge| edge.item.content.reference() == grandchild));
    drop(reader);
    let expected = timeline.clone();
    drop(store);
    let reopened = ConversationAdmissionStore::open(
        directory.path().join("nested.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    assert_eq!(
        reopened.retained_timeline(&conversation).unwrap(),
        &expected
    );
    assert_eq!(
        reopened.conversation(&conversation).unwrap().messages,
        messages
    );
}
