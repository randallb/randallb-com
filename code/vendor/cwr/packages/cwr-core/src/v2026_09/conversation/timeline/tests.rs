use super::*;
use crate::v2026_09::{conversation::ConversationMessage, invocation::ActionInvocationLedger};

const CONVERSATION: &str = "cwp://example.test/conversations/shared";
const ROOT: &str = "cwp://example.test/conversations/shared/timeline";
const INVOCATION: &str = "cwp://example.test/invocations/one";
const MESSAGE: &str = "cwp://example.test/messages/one";

fn invocation() -> ConversationTimelineContent {
    ConversationTimelineContent::Invocation {
        invocation_ref: INVOCATION.into(),
    }
}

fn message() -> ConversationTimelineContent {
    ConversationTimelineContent::Message {
        message_ref: MESSAGE.into(),
    }
}

#[test]
fn root_and_child_connections_page_with_stable_cursors() {
    let mut timeline = ConversationTimeline::new(CONVERSATION, ROOT).unwrap();
    let parent = match timeline
        .publish(
            "cwp://example.test/publications/invocation-one",
            invocation(),
            ConversationTimelineDisposition::Root,
        )
        .unwrap()
    {
        ConversationTimelinePublishOutcome::Placed(item) => item,
        outcome => panic!("unexpected publication outcome {outcome:?}"),
    };
    let root_revision = timeline.connection(ROOT).unwrap().revision;
    let initial = timeline
        .page(
            ROOT,
            &ConversationTimelinePageRequest {
                last: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    let cursor = initial.page_info.end_cursor.unwrap();

    timeline
        .publish(
            "cwp://example.test/publications/message-one",
            message(),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    let appended = timeline
        .page(
            ROOT,
            &ConversationTimelinePageRequest {
                after: Some(cursor),
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(appended.edges.len(), 1);
    assert_eq!(appended.edges[0].item.content, message());

    timeline
        .publish(
            "cwp://example.test/publications/child-message",
            ConversationTimelineContent::Message {
                message_ref: "cwp://example.test/messages/child".into(),
            },
            ConversationTimelineDisposition::Child {
                parent_item_ref: parent.item_ref.clone(),
            },
        )
        .unwrap();
    assert_eq!(
        timeline.connection(ROOT).unwrap().revision,
        root_revision + 1
    );
    let child = timeline
        .connection(parent.child_connection_ref.as_deref().unwrap())
        .unwrap();
    assert_eq!(child.revision, 1);
    assert_eq!(child.items.len(), 1);
    timeline.validate_for_conversation(CONVERSATION).unwrap();
}

#[test]
fn publication_replay_is_idempotent_but_mismatches_fail() {
    let mut timeline = ConversationTimeline::new(CONVERSATION, ROOT).unwrap();
    let publication = "cwp://example.test/publications/message-one";
    let placed = timeline
        .publish(
            publication,
            message(),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    let replayed = timeline
        .publish(
            publication,
            message(),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    assert!(matches!(
        placed,
        ConversationTimelinePublishOutcome::Placed(_)
    ));
    assert!(matches!(
        replayed,
        ConversationTimelinePublishOutcome::Existing(_)
    ));
    assert!(timeline
        .publish(
            publication,
            invocation(),
            ConversationTimelineDisposition::Root
        )
        .is_err());
}

#[test]
fn creating_a_populated_timeline_again_is_idempotent() {
    let mut store = ConversationTimelineStore::default();
    assert!(store.create(CONVERSATION, ROOT).unwrap());
    store
        .publish(
            CONVERSATION,
            "cwp://example.test/publications/message-one",
            message(),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    assert!(!store.create(CONVERSATION, ROOT).unwrap());
    assert!(store
        .create("cwp://example.test/conversations/other", ROOT)
        .is_err());
    assert!(store
        .create(
            CONVERSATION,
            "cwp://example.test/conversations/shared/other"
        )
        .is_err());
}

#[test]
fn timeline_root_cannot_reuse_its_conversation_ref() {
    let error = ConversationTimeline::new(CONVERSATION, CONVERSATION).unwrap_err();
    assert!(error
        .to_string()
        .contains("root connection must differ from its conversation"));
}

#[test]
fn store_publication_rejects_identities_owned_by_another_timeline() {
    let other_conversation = "cwp://example.test/conversations/other";
    let other_root = "cwp://example.test/conversations/other/timeline";
    let publication = "cwp://example.test/publications/cross-timeline";
    let derived_collision = "cwp://example.test/publications/derived-collision";
    let mut store = ConversationTimelineStore::default();
    store.create(CONVERSATION, ROOT).unwrap();
    store.create(other_conversation, other_root).unwrap();
    store
        .publish(
            CONVERSATION,
            publication,
            invocation(),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    for (index, existing_ref) in [
        format!("{derived_collision}/item"),
        format!("{derived_collision}/item/children"),
    ]
    .into_iter()
    .enumerate()
    {
        store
            .publish(
                CONVERSATION,
                existing_ref,
                ConversationTimelineContent::Message {
                    message_ref: format!("cwp://example.test/messages/existing-{index}"),
                },
                ConversationTimelineDisposition::Root,
            )
            .unwrap();
    }

    for (index, collision_ref) in [
        CONVERSATION.to_owned(),
        publication.to_owned(),
        format!("{publication}/item"),
        format!("{publication}/item/children"),
    ]
    .into_iter()
    .enumerate()
    {
        let error = store
            .publish(
                other_conversation,
                collision_ref,
                ConversationTimelineContent::Message {
                    message_ref: format!("cwp://example.test/messages/cross-{index}"),
                },
                ConversationTimelineDisposition::Root,
            )
            .unwrap_err();
        assert!(error.to_string().contains("another timeline"), "{error}");
    }

    let error = store
        .publish(
            other_conversation,
            "cwp://example.test/publications/cross-content",
            ConversationTimelineContent::Message {
                message_ref: CONVERSATION.into(),
            },
            ConversationTimelineDisposition::Root,
        )
        .unwrap_err();
    assert!(error.to_string().contains("another timeline"), "{error}");

    for content in [
        ConversationTimelineContent::Message {
            message_ref: "cwp://example.test/messages/derived-item".into(),
        },
        ConversationTimelineContent::Invocation {
            invocation_ref: "cwp://example.test/invocations/derived-child".into(),
        },
    ] {
        let error = store
            .publish(
                other_conversation,
                derived_collision,
                content,
                ConversationTimelineDisposition::Root,
            )
            .unwrap_err();
        assert!(error.to_string().contains("another timeline"), "{error}");
    }
}

#[test]
fn new_records_cannot_reuse_disclosed_or_undisclosed_timeline_identities() {
    let disclosed_ref = "cwp://example.test/publications/disclosed";
    let undisclosed_ref = "cwp://example.test/publications/undisclosed";
    let source_invocation_ref = "cwp://example.test/invocations/source";
    let source_message: ConversationMessage = serde_json::from_value(serde_json::json!({
        "message_ref": MESSAGE,
        "author_actor_ref": "cwp://example.test/actors/person",
        "addressed_actor_refs": [],
        "text": "source"
    }))
    .unwrap();
    let conversation = RetainedConversation {
        owner_context_ref: None,
        conversation_ref: CONVERSATION.into(),
        title: None,
        archived: false,
        parent_conversation_ref: None,
        sidebar_eligible: None,
        participant_actor_refs: Vec::new(),
        messages: vec![source_message],
        timeline_connection_ref: Some(ROOT.into()),
        activity: Vec::new(),
    };
    let mut timeline_store = ConversationTimelineStore::default();
    timeline_store.create(CONVERSATION, ROOT).unwrap();
    timeline_store
        .publish(
            CONVERSATION,
            disclosed_ref,
            message(),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    let mut invocation_ledger = ActionInvocationLedger::default();
    invocation_ledger
        .start(crate::v2026_09::invocation::StartActionInvocation {
            invocation_ref: source_invocation_ref.into(),
            origin: crate::v2026_09::invocation::InvocationOrigin::Ingress {
                integration_ref: "cwp://example.test/integrations/test".into(),
                provider_event_ref: "source".into(),
            },
            requester_actor_ref: "cwp://example.test/actors/person".into(),
            target_actor_ref: "cwp://example.test/actors/assistant".into(),
            conversation_ref: Some(CONVERSATION.into()),
            actor_integration_ref: None,
            action_name: "respond_to--message".into(),
            input: serde_json::json!({}),
            parent_invocation_ref: None,
            admitted_at_unix_ms: 1,
        })
        .unwrap();
    invocation_ledger
        .retain_timeline_publication(
            source_invocation_ref,
            MESSAGE,
            undisclosed_ref,
            ConversationTimelineDisposition::Undisclosed,
        )
        .unwrap();
    let state = ConversationRuntimeState {
        cold_conversation_refs: Default::default(),
        files: Default::default(),
        streaming_messages: Default::default(),
        streaming_runs: Default::default(),
        tool_attempts: Default::default(),
        tool_operations: Default::default(),
        version: super::super::STATE_VERSION.into(),
        runtime_root_ref: "cwp://example.test".into(),
        conversations: vec![conversation].into(),
        visualization_artifacts: Default::default(),
        admissions: Default::default(),
        invocation_ledger,
        timeline_store,
        invocation_execution_conversation_refs: Default::default(),
        upstream_participant_versions: Default::default(),
        protected_result_children: Default::default(),
    };

    let mut unmatched_item = state.clone();
    unmatched_item
        .timeline_store
        .publish(
            CONVERSATION,
            "cwp://example.test/publications/unmatched-invocation",
            ConversationTimelineContent::Invocation {
                invocation_ref: source_invocation_ref.into(),
            },
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    let error = validate_retained_timelines(&unmatched_item).unwrap_err();
    assert!(error
        .to_string()
        .contains("no retained invocation publication decision"));

    for (index, collision_ref) in [disclosed_ref, undisclosed_ref].into_iter().enumerate() {
        let mut changed = state.clone();
        changed.begin();
        changed
            .invocation_ledger
            .start(crate::v2026_09::invocation::StartActionInvocation {
                invocation_ref: collision_ref.into(),
                origin: crate::v2026_09::invocation::InvocationOrigin::Ingress {
                    integration_ref: "cwp://example.test/integrations/test".into(),
                    provider_event_ref: format!("collision-{index}"),
                },
                requester_actor_ref: "cwp://example.test/actors/person".into(),
                target_actor_ref: "cwp://example.test/actors/assistant".into(),
                conversation_ref: Some(CONVERSATION.into()),
                actor_integration_ref: None,
                action_name: "respond_to--message".into(),
                input: serde_json::json!({}),
                parent_invocation_ref: None,
                admitted_at_unix_ms: 2 + u64::try_from(index).unwrap(),
            })
            .unwrap();
        let error = validate_changed_timelines(&changed).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("reuses a retained timeline identity"),
            "{error}"
        );
    }

    for collision_ref in [disclosed_ref, undisclosed_ref] {
        let mut changed = state.clone();
        changed.begin();
        changed.conversations.push(RetainedConversation {
            owner_context_ref: None,
            conversation_ref: collision_ref.into(),
            title: None,
            archived: false,
            parent_conversation_ref: None,
            sidebar_eligible: None,
            participant_actor_refs: Vec::new(),
            messages: Vec::new(),
            timeline_connection_ref: None,
            activity: Vec::new(),
        });
        let error = validate_changed_timelines(&changed).unwrap_err();
        assert!(
            error.to_string().contains("new conversation")
                && error
                    .to_string()
                    .contains("reuses a retained timeline identity"),
            "{error}"
        );
    }
}

#[test]
fn the_same_content_can_have_multiple_placements() {
    let mut timeline = ConversationTimeline::new(CONVERSATION, ROOT).unwrap();
    for publication in [
        "cwp://example.test/publications/message-one",
        "cwp://example.test/publications/message-two",
    ] {
        timeline
            .publish(
                publication,
                message(),
                ConversationTimelineDisposition::Root,
            )
            .unwrap();
    }
    let root = timeline.connection(ROOT).unwrap();
    assert_eq!(root.items.len(), 2);
    assert_ne!(root.items[0].item_ref, root.items[1].item_ref);
    assert_eq!(root.items[0].content, root.items[1].content);
}

#[test]
fn undisclosed_decisions_create_no_placement() {
    let mut timeline = ConversationTimeline::new(CONVERSATION, ROOT).unwrap();
    let outcome = timeline
        .publish(
            "cwp://example.test/publications/private",
            invocation(),
            ConversationTimelineDisposition::Undisclosed,
        )
        .unwrap();
    assert_eq!(outcome, ConversationTimelinePublishOutcome::Undisclosed);
    assert!(timeline.connection(ROOT).unwrap().items.is_empty());
    assert_eq!(timeline.connection(ROOT).unwrap().revision, 0);
}

#[test]
fn cursors_are_connection_local() {
    let mut timeline = ConversationTimeline::new(CONVERSATION, ROOT).unwrap();
    let parent = match timeline
        .publish(
            "cwp://example.test/publications/parent",
            invocation(),
            ConversationTimelineDisposition::Root,
        )
        .unwrap()
    {
        ConversationTimelinePublishOutcome::Placed(item) => item,
        outcome => panic!("unexpected publication outcome {outcome:?}"),
    };
    let cursor = timeline
        .page(
            ROOT,
            &ConversationTimelinePageRequest {
                last: Some(1),
                ..Default::default()
            },
        )
        .unwrap()
        .page_info
        .end_cursor
        .unwrap();
    assert!(timeline
        .page(
            parent.child_connection_ref.as_deref().unwrap(),
            &ConversationTimelinePageRequest {
                after: Some(cursor),
                first: Some(1),
                ..Default::default()
            },
        )
        .is_err());
}

#[test]
fn page_flags_are_relative_to_the_cursor_bounded_range() {
    let mut timeline = ConversationTimeline::new(CONVERSATION, ROOT).unwrap();
    for index in 1..=3 {
        timeline
            .publish(
                format!("cwp://example.test/publications/message-{index}"),
                message(),
                ConversationTimelineDisposition::Root,
            )
            .unwrap();
    }
    let all = timeline
        .page(
            ROOT,
            &ConversationTimelinePageRequest {
                first: Some(3),
                ..Default::default()
            },
        )
        .unwrap();
    let after_first = timeline
        .page(
            ROOT,
            &ConversationTimelinePageRequest {
                after: Some(all.edges[0].cursor.clone()),
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!after_first.page_info.has_previous_page);
    assert!(after_first.page_info.has_next_page);

    let before_last = timeline
        .page(
            ROOT,
            &ConversationTimelinePageRequest {
                before: Some(all.edges[2].cursor.clone()),
                last: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(before_last.page_info.has_previous_page);
    assert!(!before_last.page_info.has_next_page);
}

#[test]
fn retained_conversation_distinguishes_legacy_absence_from_an_empty_timeline() {
    let legacy: RetainedConversation = serde_json::from_value(serde_json::json!({
        "conversation_ref": CONVERSATION,
        "participant_actor_refs": [],
        "messages": []
    }))
    .unwrap();
    assert!(legacy.timeline_connection_ref.is_none());

    let mut authoritative = legacy.clone();
    authoritative.timeline_connection_ref = Some(ROOT.into());
    let reopened: RetainedConversation =
        serde_json::from_value(serde_json::to_value(authoritative).unwrap()).unwrap();
    assert_eq!(reopened.timeline_connection_ref.as_deref(), Some(ROOT));
}

#[test]
fn private_execution_conversations_cannot_own_shared_timelines() {
    let conversation = RetainedConversation {
        owner_context_ref: None,
        conversation_ref: CONVERSATION.into(),
        title: None,
        archived: false,
        parent_conversation_ref: Some("cwp://example.test/conversations/parent".into()),
        sidebar_eligible: None,
        participant_actor_refs: Vec::new(),
        messages: Vec::new(),
        timeline_connection_ref: Some(ROOT.into()),
        activity: Vec::new(),
    };
    let mut timeline_store = ConversationTimelineStore::default();
    timeline_store.create(CONVERSATION, ROOT).unwrap();
    let state = ConversationRuntimeState {
        cold_conversation_refs: Default::default(),
        files: Default::default(),
        streaming_messages: Default::default(),
        streaming_runs: Default::default(),
        tool_attempts: Default::default(),
        tool_operations: Default::default(),
        version: super::super::STATE_VERSION.into(),
        runtime_root_ref: "cwp://example.test".into(),
        conversations: vec![conversation].into(),
        visualization_artifacts: Default::default(),
        admissions: Default::default(),
        invocation_ledger: Default::default(),
        timeline_store,
        invocation_execution_conversation_refs: Default::default(),
        upstream_participant_versions: Default::default(),
        protected_result_children: Default::default(),
    };
    assert!(validate_retained_timelines(&state).is_err());
}
