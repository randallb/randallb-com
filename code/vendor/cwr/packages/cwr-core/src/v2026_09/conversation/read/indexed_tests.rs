use std::time::Duration;

use cwpdb_store::test_support::indexed_history::IndexedFixture;
use serde_json::json;

use super::*;
use crate::v2026_09::conversation::{ConversationAdmissionStore, ConversationReadStore};

const RUNTIME: &str = "cwp://com.example.runtime";
const CONVERSATION: &str = "cwp://com.example.runtime/conversations/indexed";
const CHILD_CONVERSATION: &str = "cwp://com.example.runtime/conversations/indexed-child";

#[path = "indexed_dependency_tests.rs"]
mod dependency_tests;
#[path = "indexed_fixtures.rs"]
mod fixtures;

fn seeded_store() -> (tempfile::TempDir, std::path::PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite");
    drop(ConversationAdmissionStore::open(&path, RUNTIME).unwrap());
    let connection = IndexedFixture::new(&path, RUNTIME, CONVERSATION);
    fixtures::conversation(&connection, CONVERSATION, 0, "Indexed", None, 7);
    for position in 0..4_i64 {
        let message_ref = format!("{RUNTIME}/messages/{position}");
        let payload = serde_json::to_vec(&json!({
            "message_ref": message_ref,
            "author_actor_ref": format!("{RUNTIME}/actors/person"),
            "addressed_actor_refs": [],
            "text": format!("Message {position}")
        }))
        .unwrap();
        fixtures::message(
            &connection,
            position,
            &serde_json::from_slice(&payload).unwrap(),
        );
    }
    let progress_ref = format!("{RUNTIME}/messages/progress");
    let progress_payload = serde_json::to_vec(&json!({
        "progress": {
            "invocation_ref": format!("{RUNTIME}/invocations/one"),
            "source_activity_ref": format!("{RUNTIME}/activities/2")
        },
        "message_ref": progress_ref,
        "author_actor_ref": format!("{RUNTIME}/actors/coworker"),
        "addressed_actor_refs": [],
        "text": "Published progress"
    }))
    .unwrap();
    fixtures::message(
        &connection,
        4,
        &serde_json::from_slice(&progress_payload).unwrap(),
    );
    let activities = [
        json!({
            "activity_ref": format!("{RUNTIME}/activities/0"), "sequence": 0,
            "occurred_at_unix_ms": 10,
            "event": {"kind": "message_recorded", "message_ref": format!("{RUNTIME}/messages/0")}
        }),
        json!({
            "activity_ref": format!("{RUNTIME}/activities/1"), "sequence": 1,
            "occurred_at_unix_ms": 20,
            "event": {"kind": "turn_started", "invocation_ref": format!("{RUNTIME}/invocations/one"),
                "source_message_ref": format!("{RUNTIME}/messages/0")}
        }),
        json!({
            "activity_ref": format!("{RUNTIME}/activities/2"), "sequence": 2,
            "occurred_at_unix_ms": 30,
            "event": {"kind": "work_update", "invocation_ref": format!("{RUNTIME}/invocations/one"),
                "source_message_ref": format!("{RUNTIME}/messages/0"), "actor_ref": format!("{RUNTIME}/actors/coworker"),
                "status": 102, "text": "Working"}
        }),
        json!({
            "activity_ref": format!("{RUNTIME}/activities/3"), "sequence": 3,
            "occurred_at_unix_ms": 40,
            "event": {"kind": "message_recorded", "message_ref": format!("{RUNTIME}/messages/1")}
        }),
        json!({
            "activity_ref": format!("{RUNTIME}/activities/4"), "sequence": 4,
            "occurred_at_unix_ms": 50,
            "event": {"kind": "message_recorded", "message_ref": format!("{RUNTIME}/messages/2")}
        }),
    ];
    for (position, activity) in activities.into_iter().enumerate() {
        fixtures::record(&connection, "activity", position as i64, &activity);
    }
    for (position, (kind, reference)) in [
        ("message", format!("{RUNTIME}/messages/0")),
        ("turn", format!("{RUNTIME}/invocations/one")),
        ("message", format!("{RUNTIME}/messages/1")),
        ("message", format!("{RUNTIME}/messages/2")),
    ]
    .into_iter()
    .enumerate()
    {
        fixtures::anchor(&connection, position as i64, kind, &reference);
    }
    let public_update = json!({
        "activity_ref": format!("{RUNTIME}/activities/2"), "sequence": 2,
        "occurred_at_unix_ms": 30,
        "event": {"kind": "work_update", "invocation_ref": format!("{RUNTIME}/invocations/one"),
            "source_message_ref": format!("{RUNTIME}/messages/0"), "actor_ref": format!("{RUNTIME}/actors/coworker"),
            "status": 102, "text": "Working"}
    });
    fixtures::public_activity(&connection, 0, &public_update);
    connection.commit().unwrap();
    (temporary, path)
}

#[test]
fn pages_visible_content_from_one_snapshot() {
    let (_temporary, path) = seeded_store();
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(Duration::from_secs(2)).unwrap();

    let content = operation
        .conversation_content(
            CONVERSATION,
            ConversationRecordWindow {
                first: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(content.revision, "7");
    assert!(content
        .content
        .iter()
        .all(|item| item.key.revision.is_none()));
    assert_eq!(content.total_count, 4);
    assert_eq!(
        content
            .content
            .iter()
            .map(|item| item.message.text.as_str())
            .collect::<Vec<_>>(),
        vec!["Message 0", "Message 1"]
    );
    assert_eq!(
        content
            .content
            .iter()
            .map(|item| item.occurred_at_unix_ms)
            .collect::<Vec<_>>(),
        vec![Some(10), Some(40)]
    );
    assert!(content.has_next_page);
}

#[test]
fn pages_timeline_anchors_with_complete_turn_dependencies() {
    let (_temporary, path) = seeded_store();
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(Duration::from_secs(2)).unwrap();

    let timeline = operation
        .conversation_timeline(
            CONVERSATION,
            ConversationRecordWindow {
                first: Some(2),
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(timeline.revision, "7");
    assert!(timeline.anchors.iter().all(|anchor| match anchor {
        ConversationTimelineAnchor::Message { key, .. }
        | ConversationTimelineAnchor::Turn { key, .. } => key.revision.as_deref() == Some("7"),
    }));
    assert_eq!(timeline.total_count, 4);
    assert_eq!(
        timeline
            .anchors
            .iter()
            .map(timeline_anchor_position)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert!(timeline.has_next_page);
    assert!(!timeline.has_previous_page);
    assert_eq!(
        timeline
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>(),
        vec!["Message 0"]
    );
    assert_eq!(
        timeline
            .activity
            .iter()
            .map(|activity| activity.activity_ref.as_str())
            .collect::<Vec<_>>(),
        vec![
            "cwp://com.example.runtime/activities/0",
            "cwp://com.example.runtime/activities/2"
        ]
    );
}

#[test]
fn timeline_page_boundaries_use_the_visible_relation() {
    let (_temporary, path) = seeded_store();
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(Duration::from_secs(2)).unwrap();

    let page = operation
        .conversation_timeline(
            CONVERSATION,
            ConversationRecordWindow {
                last: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        page.anchors
            .iter()
            .map(timeline_anchor_position)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert!(page.has_previous_page);
    assert!(!page.has_next_page);
    assert_eq!(page.total_count, 4);
    let revision = page.revision.clone();

    let after = ConversationPositionKey {
        position: 1,
        revision: Some(revision.clone()),
    };
    let next = operation
        .conversation_timeline(
            CONVERSATION,
            ConversationRecordWindow {
                after: Some(&after),
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        next.anchors
            .iter()
            .map(timeline_anchor_position)
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert!(next.has_next_page);
    assert!(next.has_previous_page);

    let after_first = ConversationPositionKey {
        position: 0,
        revision: Some(revision.clone()),
    };
    let after_first_page = operation
        .conversation_timeline(
            CONVERSATION,
            ConversationRecordWindow {
                after: Some(&after_first),
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!after_first_page.has_previous_page);

    let before = ConversationPositionKey {
        position: 3,
        revision: Some(revision.clone()),
    };
    let previous = operation
        .conversation_timeline(
            CONVERSATION,
            ConversationRecordWindow {
                before: Some(&before),
                last: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        previous
            .anchors
            .iter()
            .map(timeline_anchor_position)
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert!(!previous.has_next_page);
    assert!(previous.has_previous_page);

    let before_last = ConversationPositionKey {
        position: 2,
        revision: Some(revision.clone()),
    };
    let before_last_page = operation
        .conversation_timeline(
            CONVERSATION,
            ConversationRecordWindow {
                before: Some(&before_last),
                last: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(before_last_page.has_next_page);

    let empty = operation
        .conversation_timeline(
            CONVERSATION,
            ConversationRecordWindow {
                first: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(empty.anchors.is_empty());
    assert!(empty.has_next_page);
    assert!(!empty.has_previous_page);

    let earlier_revision = ConversationPositionKey {
        position: 1,
        revision: Some("6".into()),
    };
    assert!(operation
        .conversation_timeline(
            CONVERSATION,
            ConversationRecordWindow {
                after: Some(&earlier_revision),
                first: Some(1),
                ..Default::default()
            },
        )
        .is_err());

    let stale = ConversationPositionKey {
        position: 9,
        revision: Some(revision),
    };
    assert!(operation
        .conversation_timeline(
            CONVERSATION,
            ConversationRecordWindow {
                after: Some(&stale),
                first: Some(1),
                ..Default::default()
            },
        )
        .is_err());
}

#[test]
fn rejects_hidden_child_conversations() {
    let (_temporary, path) = seeded_store();
    let connection = IndexedFixture::new(&path, RUNTIME, CONVERSATION);
    fixtures::conversation(
        &connection,
        CHILD_CONVERSATION,
        1,
        "Child",
        Some(CONVERSATION),
        1,
    );
    connection.commit().unwrap();

    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(Duration::from_secs(2)).unwrap();
    let error = operation
        .conversation_content(
            CHILD_CONVERSATION,
            ConversationRecordWindow {
                first: Some(1),
                ..Default::default()
            },
        )
        .expect_err("child execution conversations are not host-readable");

    assert!(error.to_string().contains("unknown conversation ref"));
}

#[test]
fn indexed_pages_hold_revision_and_validate_relation_cursors() {
    let (_temporary, path) = seeded_store();
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(Duration::from_secs(2)).unwrap();
    cwpdb_store::test_support::indexed_history::commit_revision(&path, RUNTIME, CONVERSATION, 8)
        .unwrap();
    let page = operation
        .conversation_content(
            CONVERSATION,
            ConversationRecordWindow {
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(page.revision, "7");
    assert!(operation
        .conversation_content(
            CONVERSATION,
            ConversationRecordWindow {
                after: Some(&ConversationPositionKey {
                    position: 10,
                    revision: None,
                }),
                first: Some(1),
                ..Default::default()
            },
        )
        .is_err());
}

#[test]
fn content_cursors_survive_timeline_only_revision_changes() {
    let (_temporary, path) = seeded_store();
    let cursor = {
        let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
        let operation = store.begin_operation(Duration::from_secs(2)).unwrap();
        operation
            .conversation_content(
                CONVERSATION,
                ConversationRecordWindow {
                    first: Some(1),
                    ..Default::default()
                },
            )
            .unwrap()
            .content[0]
            .key
            .clone()
    };
    assert_eq!(cursor.revision, None);
    cwpdb_store::test_support::indexed_history::commit_revision(&path, RUNTIME, CONVERSATION, 8)
        .unwrap();

    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(Duration::from_secs(2)).unwrap();
    let page = operation
        .conversation_content(
            CONVERSATION,
            ConversationRecordWindow {
                after: Some(&cursor),
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(page.revision, "8");
    assert_eq!(page.content[0].message.text, "Message 1");
}

#[test]
fn page_info_is_relative_to_cursor_filtered_content() {
    let (_temporary, path) = seeded_store();
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(Duration::from_secs(2)).unwrap();
    let after = ConversationPositionKey {
        position: 0,
        revision: None,
    };
    let forward = operation
        .conversation_content(
            CONVERSATION,
            ConversationRecordWindow {
                after: Some(&after),
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(forward.content[0].key.position, 1);
    assert!(!forward.has_previous_page);
    assert!(forward.has_next_page);

    let before = ConversationPositionKey {
        position: 3,
        revision: None,
    };
    let backward = operation
        .conversation_content(
            CONVERSATION,
            ConversationRecordWindow {
                before: Some(&before),
                last: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(backward.content[0].key.position, 2);
    assert!(backward.has_previous_page);
    assert!(!backward.has_next_page);
}

fn timeline_anchor_position(anchor: &ConversationTimelineAnchor) -> i64 {
    match anchor {
        ConversationTimelineAnchor::Message { key, .. }
        | ConversationTimelineAnchor::Turn { key, .. } => key.position,
    }
}
