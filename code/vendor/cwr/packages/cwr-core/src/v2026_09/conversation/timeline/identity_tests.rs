use super::*;

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
fn publication_mutation_rejects_identity_collisions() {
    let mut timeline = ConversationTimeline::new(CONVERSATION, ROOT).unwrap();
    assert!(timeline
        .publish(ROOT, message(), ConversationTimelineDisposition::Root)
        .is_err());
    assert!(timeline
        .publish(
            CONVERSATION,
            message(),
            ConversationTimelineDisposition::Root
        )
        .is_err());
    assert!(timeline
        .publish(
            "cwp://example.test/publications/conversation-content",
            ConversationTimelineContent::Message {
                message_ref: CONVERSATION.into(),
            },
            ConversationTimelineDisposition::Root,
        )
        .is_err());

    let publication = "cwp://example.test/publications/message-one";
    let placed = match timeline
        .publish(
            publication,
            message(),
            ConversationTimelineDisposition::Root,
        )
        .unwrap()
    {
        ConversationTimelinePublishOutcome::Placed(item) => item,
        outcome => panic!("unexpected publication outcome {outcome:?}"),
    };
    assert!(timeline
        .publish(
            &placed.item_ref,
            invocation(),
            ConversationTimelineDisposition::Root,
        )
        .is_err());
    assert!(timeline
        .publish(MESSAGE, invocation(), ConversationTimelineDisposition::Root,)
        .is_err());
    assert!(timeline
        .publish(
            "cwp://example.test/publications/content-collision",
            ConversationTimelineContent::Message {
                message_ref: publication.into(),
            },
            ConversationTimelineDisposition::Root,
        )
        .is_err());

    let derived_collision = "cwp://example.test/publications/derived/item";
    timeline
        .publish(
            derived_collision,
            message(),
            ConversationTimelineDisposition::Root,
        )
        .unwrap();
    assert!(timeline
        .publish(
            "cwp://example.test/publications/derived",
            message(),
            ConversationTimelineDisposition::Root,
        )
        .is_err());

    let derived_conversation = "cwp://example.test/publications/conversation-item/item";
    let mut derived_timeline = ConversationTimeline::new(
        derived_conversation,
        "cwp://example.test/publications/conversation-item/root",
    )
    .unwrap();
    assert!(derived_timeline
        .publish(
            "cwp://example.test/publications/conversation-item",
            message(),
            ConversationTimelineDisposition::Root,
        )
        .is_err());
    timeline.validate_for_conversation(CONVERSATION).unwrap();
}

#[test]
fn store_creation_reserves_conversation_refs_across_timelines() {
    let publication = "cwp://example.test/publications/invocation";
    let mut store = ConversationTimelineStore::default();
    store.create(CONVERSATION, ROOT).unwrap();
    let placed = match store
        .publish(
            CONVERSATION,
            publication,
            invocation(),
            ConversationTimelineDisposition::Root,
        )
        .unwrap()
    {
        ConversationTimelinePublishOutcome::Placed(item) => item,
        outcome => panic!("unexpected publication outcome {outcome:?}"),
    };

    for (index, conversation_ref) in [
        ROOT,
        publication,
        placed.item_ref.as_str(),
        placed.child_connection_ref.as_deref().unwrap(),
        INVOCATION,
    ]
    .into_iter()
    .enumerate()
    {
        let error = store
            .create(
                conversation_ref,
                format!("cwp://example.test/timelines/new-{index}"),
            )
            .unwrap_err();
        assert!(error.to_string().contains("another timeline"), "{error}");
    }
}
