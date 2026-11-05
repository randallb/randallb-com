use super::*;
use crate::v2026_09::conversation::{ConversationAdmissionStore, ConversationReadStore};
use std::time::Duration;

const RUNTIME: &str = "cwp://example.test/runtime";
const CONVERSATION: &str = "cwp://example.test/conversations/legacy";

#[test]
fn legacy_pages_resolve_only_exact_current_anchors() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.sqlite");
    let mut retained = ConversationAdmissionStore::open(&path, RUNTIME).unwrap();
    let mut state = retained.state.clone();
    state.conversations = vec![serde_json::from_value(serde_json::json!({
        "conversation_ref": CONVERSATION,
        "participant_actor_refs": [],
        "messages": (0..5).map(|index| serde_json::json!({
            "message_ref": format!("{CONVERSATION}/messages/{index}"),
            "author_actor_ref": "cwp://example.test/actors/person", "addressed_actor_refs": [],
            "text": format!("Message {index}")
        })).collect::<Vec<_>>()
    }))
    .unwrap()]
    .into();
    retained.replace_state(state).unwrap();
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(Duration::from_secs(5)).unwrap();
    assert_eq!(
        operation
            .conversation_timeline_layout(CONVERSATION)
            .unwrap(),
        None
    );
    let page = operation
        .conversation_legacy_timeline_page(
            CONVERSATION,
            &ConversationRecordPageRequest {
                first: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(page.anchors.len(), 2);
    assert!(!page.has_previous_page);
    assert!(page.has_next_page);
    let key = match &page.anchors[1] {
        ConversationTimelineAnchor::Message { key, message_ref } => {
            assert_eq!(
                operation
                    .conversation_legacy_timeline_message(CONVERSATION, key, message_ref)
                    .unwrap()
                    .unwrap()
                    .text,
                "Message 1"
            );
            assert!(operation
                .conversation_legacy_timeline_message(
                    CONVERSATION,
                    key,
                    "cwp://example.test/messages/unrelated"
                )
                .unwrap()
                .is_none());
            key.clone()
        }
        _ => panic!("expected message anchor"),
    };
    let next = operation
        .conversation_legacy_timeline_page(
            CONVERSATION,
            &ConversationRecordPageRequest {
                after: Some(key.clone()),
                first: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(next.has_previous_page && next.has_next_page);
    let last = operation
        .conversation_legacy_timeline_page(
            CONVERSATION,
            &ConversationRecordPageRequest {
                last: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(last.has_previous_page && !last.has_next_page);
    for request in [
        ConversationRecordPageRequest {
            first: Some(0),
            ..Default::default()
        },
        ConversationRecordPageRequest {
            first: Some(501),
            ..Default::default()
        },
        ConversationRecordPageRequest {
            first: Some(1),
            last: Some(1),
            ..Default::default()
        },
        ConversationRecordPageRequest {
            after: Some(ConversationPositionKey {
                revision: None,
                ..key.clone()
            }),
            first: Some(1),
            ..Default::default()
        },
    ] {
        assert!(operation
            .conversation_legacy_timeline_page(CONVERSATION, &request)
            .is_err());
    }
    for (boundary, after) in [(0, false), (4, true)] {
        let cursor = ConversationPositionKey {
            position: boundary,
            revision: Some(page.revision.clone()),
        };
        let request = if after {
            ConversationRecordPageRequest {
                after: Some(cursor),
                first: Some(2),
                ..Default::default()
            }
        } else {
            ConversationRecordPageRequest {
                before: Some(cursor),
                last: Some(2),
                ..Default::default()
            }
        };
        let empty = operation
            .conversation_legacy_timeline_page(CONVERSATION, &request)
            .unwrap();
        assert!(empty.anchors.is_empty());
        assert_eq!(empty.has_previous_page, after);
        assert_eq!(empty.has_next_page, !after);
    }
    let mut stale = key.clone();
    stale.revision = Some("obsolete".into());
    assert!(operation
        .conversation_legacy_timeline_page(
            CONVERSATION,
            &ConversationRecordPageRequest {
                after: Some(stale),
                first: Some(2),
                ..Default::default()
            }
        )
        .is_err());
    let missing = ConversationPositionKey {
        position: 100,
        ..key
    };
    assert!(operation
        .conversation_legacy_timeline_message(CONVERSATION, &missing, "anything")
        .is_err());
    assert!(operation
        .conversation_timeline_layout("cwp://example.test/missing")
        .is_err());
}
