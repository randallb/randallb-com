use super::*;
use std::collections::BTreeSet;

#[test]
fn timeline_context_does_not_hydrate_sibling_work_as_history_grows() {
    for branches in [1, 32, 128] {
        let (_temporary, path) = seeded_store();
        let connection = IndexedFixture::new(&path, RUNTIME, CONVERSATION);
        let root = format!("{RUNTIME}/invocations/one");
        let parent = format!("{RUNTIME}/invocations/parent");
        let parent_source = format!("{RUNTIME}/messages/parent");
        let root_source = format!("{RUNTIME}/messages/0");
        insert_context_message(&connection, 10, &parent_source, Some(&root_source));
        let mut position = 10;
        insert_branch_work(
            &connection,
            &mut position,
            &root,
            None,
            &root_source,
            Some(&parent),
        );
        for branch in 0..branches {
            let leaf = format!("{RUNTIME}/invocations/leaf-{branch}");
            let source = format!("{RUNTIME}/messages/leaf-{branch}");
            insert_context_message(&connection, 100 + branch, &source, Some(&parent_source));
            insert_branch_work(
                &connection,
                &mut position,
                &parent,
                Some(&root),
                &parent_source,
                Some(&leaf),
            );
            for _ in 0..20 {
                insert_branch_work(
                    &connection,
                    &mut position,
                    &leaf,
                    Some(&parent),
                    &source,
                    None,
                );
            }
            fixtures::anchor(&connection, branch + 4, "turn", &leaf);
        }
        connection.commit().unwrap();
        let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
        let operation = store.begin_operation(Duration::from_secs(2)).unwrap();
        let page = operation
            .conversation_timeline(
                CONVERSATION,
                ConversationRecordWindow {
                    last: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.total_count, usize::try_from(branches + 4).unwrap());
        assert_eq!(page.anchors.len(), 1);
        assert_eq!(page.messages.len(), 3, "only source and reply ancestry");
        assert_eq!(
            page.activity.len(),
            23,
            "20 selected updates, two ancestor records, and one message timestamp, regardless of sibling history"
        );
        let invocations = page
            .activity
            .iter()
            .filter_map(activity_invocation)
            .collect::<BTreeSet<_>>();
        assert_eq!(invocations.len(), 3);
        assert!(invocations.contains(&format!("{RUNTIME}/invocations/leaf-{}", branches - 1)));
    }
}

#[test]
fn timeline_reply_ancestry_is_complete_and_cycle_safe() {
    let (_temporary, path) = seeded_store();
    let connection = IndexedFixture::new(&path, RUNTIME, CONVERSATION);
    for position in 0..3 {
        let reference = format!("{RUNTIME}/messages/{position}");
        let target = format!("{RUNTIME}/messages/{}", (position + 2) % 3);
        connection.reply(&reference, &target);
    }
    connection.commit().unwrap();
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(Duration::from_secs(2)).unwrap();
    let page = operation
        .conversation_timeline(
            CONVERSATION,
            ConversationRecordWindow {
                last: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(page.anchors.len(), 1);
    assert_eq!(page.messages.len(), 3);
    assert_eq!(page.activity.len(), 4);
}

fn insert_context_message(
    connection: &IndexedFixture,
    position: i64,
    reference: &str,
    reply: Option<&str>,
) {
    let payload = json!({"message_ref": reference, "author_actor_ref": "~@/actors/person", "addressed_actor_refs": [], "text": "Context", "reply_to_message_ref": reply});
    fixtures::message(connection, position, &payload);
}

fn insert_branch_work(
    connection: &IndexedFixture,
    position: &mut i64,
    invocation: &str,
    parent: Option<&str>,
    source: &str,
    child: Option<&str>,
) {
    let reference = format!("{RUNTIME}/activities/branch-{position}");
    let work = child.map(|child| json!({
        "activity_ref": reference, "kind": "tool", "label": "Nested work", "state": "running", "started_at_unix_ms": 1, "child_invocation_ref": child
    }));
    let payload = json!({
        "activity_ref": reference, "sequence": position, "occurred_at_unix_ms": position,
        "event": {"kind": "work_update", "invocation_ref": invocation, "source_message_ref": source,
            "parent_invocation_ref": parent, "actor_ref": "~@/actors/worker", "status": 102, "text": "Progress", "activity": work}
    });
    fixtures::public_activity(connection, *position, &payload);
    *position += 1;
}

#[test]
fn timeline_pages_keep_large_turn_activity_graphs_readable() {
    let (_temporary, path) = seeded_store();
    let connection = IndexedFixture::new(&path, RUNTIME, CONVERSATION);
    for sequence in 3..=2_003_u64 {
        let activity = json!({
            "activity_ref": format!("{RUNTIME}/activities/{sequence}"),
            "sequence": sequence,
            "occurred_at_unix_ms": sequence,
            "event": {
                "kind": "work_update",
                "invocation_ref": format!("{RUNTIME}/invocations/one"),
                "source_message_ref": format!("{RUNTIME}/messages/0"),
                "actor_ref": format!("{RUNTIME}/actors/coworker"),
                "status": 102,
                "text": format!("Progress {sequence}")
            }
        });
        fixtures::public_activity(&connection, sequence as i64, &activity);
    }
    connection.commit().unwrap();

    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(Duration::from_secs(2)).unwrap();
    let page = operation
        .conversation_timeline(
            CONVERSATION,
            ConversationRecordWindow {
                after: Some(&ConversationPositionKey {
                    position: 0,
                    revision: Some("7".into()),
                }),
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();

    assert_eq!(page.anchors.len(), 1);
    assert_eq!(page.activity.len(), 2_003);
}

fn activity_invocation(
    activity: &crate::v2026_09::conversation::ConversationActivity,
) -> Option<&String> {
    use crate::v2026_09::conversation::ConversationActivityEvent;
    match &activity.event {
        ConversationActivityEvent::WorkLifecycle { invocation_ref, .. }
        | ConversationActivityEvent::WorkUpdate { invocation_ref, .. } => Some(invocation_ref),
        _ => None,
    }
}
