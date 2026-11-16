//! Canonical fork topology and disclosure compatibility.
use super::*;

#[test]
fn canonical_fork_preserves_nested_publications_at_the_selected_reply() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fork-nested.sqlite");
    let mut store = open(&path);
    store
        .configure_timeline_rollout(ConversationTimelineRollout::NewConversations)
        .unwrap();
    let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
        panic!("new admission")
    };
    prepared.conversation.activity.push(ConversationActivity {
        activity_ref: "cwp://com.example.runtime/activities/source-recorded".into(),
        sequence: 1,
        occurred_at_unix_ms: 9,
        event: ConversationActivityEvent::MessageRecorded {
            message_ref: SOURCE_MESSAGE_REF.into(),
        },
    });
    store
        .commit(ConversationPreparationOutcome::New(prepared))
        .unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let child = "cwp://com.example.runtime/invocations/fork-child";
    start_contract_child(&mut store, ROOT_INVOCATION_REF, child, "fork-child");
    store.claim_invocation(child, 21).unwrap();
    store
        .record_invocation_progress_with_publication(child, 102, "Before fork", true)
        .unwrap();
    let PublishInvocationTerminalOutcome::Accepted {
        message: Some(reply),
        ..
    } = store
        .publish_child_action_terminal(
            child,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Child done".into(),
                result: Some(json!({"text":"Child done"})),
                error: None,
            },
            "Child done".into(),
            30,
        )
        .unwrap()
    else {
        panic!("reply")
    };
    store
        .publish_root_message_completed(
            ROOT_INVOCATION_REF,
            ActionResponseStatus::COMPLETED,
            json!({"text":"Later root reply"}),
            "Later root reply".into(),
            40,
        )
        .unwrap();
    let mut graph = crate::v2026_09::graph::resolve_graph(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/contracts/agent-recovery-turn/workspace/CWR.toml"),
    )
    .unwrap();
    let person = store.conversation(CONVERSATION_REF).unwrap().messages[0]
        .author_actor_ref
        .clone();
    graph.actors[0].actor_ref = person.clone();
    let fork = store
        .fork_conversation(
            &graph,
            &person,
            CONVERSATION_REF,
            &reply.message_ref,
            "fork-nested",
        )
        .unwrap();
    assert!(
        !fork
            .messages
            .iter()
            .any(|m| m.text.contains("Later root reply"))
    );
    let records = store
        .invocation_records()
        .filter(|r| r.conversation_ref.as_deref() == Some(&fork.conversation_ref))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    let parent = records
        .iter()
        .find(|r| r.forked_from_invocation_ref.as_deref() == Some(ROOT_INVOCATION_REF))
        .unwrap();
    let child_copy = records
        .iter()
        .find(|r| r.forked_from_invocation_ref.as_deref() == Some(child))
        .unwrap();
    assert_eq!(parent.state, ActionInvocationState::Stopped);
    assert_eq!(child_copy.state, ActionInvocationState::Completed);
    assert_eq!(
        child_copy.parent_invocation_ref.as_deref(),
        Some(parent.invocation_ref.as_str())
    );
    assert_eq!(
        child_copy.terminal_response().unwrap().summary,
        "Child done"
    );
    let timeline = store.retained_timeline(&fork.conversation_ref).unwrap();
    let child_item = timeline_item_for(timeline, &child_copy.invocation_ref);
    assert!(matches!(
        timeline
            .connection(&child_item.connection_ref)
            .unwrap()
            .owner,
        ConversationTimelineConnectionOwner::TimelineItem { .. }
    ));
    drop(store);
    ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        "cwp://com.example.runtime",
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
}

#[test]
fn canonical_fork_preserves_disclosed_reply_beneath_work_with_hidden_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("hidden-source-fork.sqlite");
    let mut store = open(&path);
    let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
        panic!("new admission")
    };
    prepared.conversation.activity.push(ConversationActivity {
        activity_ref: "cwp://com.example.runtime/activities/hidden-source".into(),
        sequence: 1,
        occurred_at_unix_ms: 9,
        event: ConversationActivityEvent::MessageRecorded {
            message_ref: SOURCE_MESSAGE_REF.into(),
        },
    });
    store
        .commit(ConversationPreparationOutcome::New(prepared))
        .unwrap();
    store
        .create_retained_timeline(
            CONVERSATION_REF,
            &format!("{CONVERSATION_REF}/hidden-source-timeline"),
        )
        .unwrap();
    store
        .retain_invocation_timeline_publication(
            ROOT_INVOCATION_REF,
            SOURCE_MESSAGE_REF,
            &format!("{CONVERSATION_REF}/publications/visible-work"),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    store
        .configure_timeline_rollout(ConversationTimelineRollout::NewConversations)
        .unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let child = "cwp://com.example.runtime/invocations/hidden-source-child";
    start_contract_child(
        &mut store,
        ROOT_INVOCATION_REF,
        child,
        "hidden-source-child",
    );
    store.claim_invocation(child, 21).unwrap();
    let PublishInvocationTerminalOutcome::Accepted {
        message: Some(reply),
        ..
    } = store
        .publish_child_action_terminal(
            child,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Visible answer".into(),
                result: Some(json!({"text":"Visible answer"})),
                error: None,
            },
            "Visible answer".into(),
            30,
        )
        .unwrap()
    else {
        panic!("reply")
    };
    let mut graph = crate::v2026_09::graph::resolve_graph(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/contracts/agent-recovery-turn/workspace/CWR.toml"),
    )
    .unwrap();
    let person = store.conversation(CONVERSATION_REF).unwrap().messages[0]
        .author_actor_ref
        .clone();
    graph.actors[0].actor_ref = person.clone();
    let fork = store
        .fork_conversation(
            &graph,
            &person,
            CONVERSATION_REF,
            &reply.message_ref,
            "hidden-source",
        )
        .unwrap();
    assert!(
        !fork
            .messages
            .iter()
            .any(|m| m.message_ref == SOURCE_MESSAGE_REF)
    );
    assert!(fork.messages.iter().any(|m| m.text == "Visible answer"));
    let records = store
        .invocation_records()
        .filter(|r| r.conversation_ref.as_deref() == Some(&fork.conversation_ref))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|r| !r.is_runnable()));
    assert!(records.iter().any(|r| r.source_message_ref.is_none()));
    let timeline = store.retained_timeline(&fork.conversation_ref).unwrap();
    assert_eq!(
        timeline
            .connections
            .iter()
            .flat_map(|c| &c.items)
            .filter(|item| matches!(item.content, ConversationTimelineContent::Message { .. }))
            .count(),
        fork.messages.len()
    );
    drop(store);
    ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        "cwp://com.example.runtime",
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
}
