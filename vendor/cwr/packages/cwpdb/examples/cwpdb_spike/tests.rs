use rusqlite::params;
use serde_json::json;

use super::{bindings, demo, model::*, queries, store::*};

fn fresh() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let graph = Store::open(&temp.path().join("graph.sqlite3")).unwrap();
    (temp, graph)
}

#[test]
fn workflow_survives_reopening_and_schema_stays_generic() {
    let (temp, mut graph) = fresh();
    let schema_before: String = graph
        .connection
        .query_row("SELECT group_concat(sql) FROM sqlite_schema", [], |r| {
            r.get(0)
        })
        .unwrap();
    demo::exercise(&mut graph).unwrap();
    let schema_after: String = graph
        .connection
        .query_row("SELECT group_concat(sql) FROM sqlite_schema", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(schema_before, schema_after);
    drop(graph);
    let graph = Store::open(&temp.path().join("graph.sqlite3")).unwrap();
    let page = queries::timeline(
        &graph.connection,
        &reference("conversations", 0),
        Window::first(10),
    )
    .unwrap();
    assert_eq!(page.items.len(), 3);
    assert_eq!(
        page.items[2].reply_to_message_ref,
        Some(reference("messages", 1))
    );
    let blobs: i64 = graph
        .connection
        .query_row(
            "SELECT count(*) FROM nodes WHERE typeof(props)!='blob'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(blobs, 0);
    assert!(graph
        .connection
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query([])
        .unwrap()
        .next()
        .unwrap()
        .is_none());
}

#[test]
fn tied_positions_page_without_gaps_in_both_directions() {
    let (_temp, mut graph) = fresh();
    graph
        .write(|tx| {
            insert_node(tx, "cwp://test/root", "collection", &json!({}))?;
            for id in 0..9 {
                let target = reference("nodes", id);
                insert_node(tx, &target, "test", &json!({}))?;
                insert_edge(
                    tx,
                    "cwp://test/root",
                    "test",
                    &target,
                    (id / 3) as i64,
                    &json!({}),
                )?;
            }
            Ok(())
        })
        .unwrap();
    let all = queries::walk(
        &graph.connection,
        "cwp://test/root",
        "test",
        Direction::Out,
        Window::first(50),
    )
    .unwrap();
    for order in [Order::Forward, Order::Backward] {
        let mut cursor = None;
        let mut seen = Vec::new();
        loop {
            let page = queries::walk(
                &graph.connection,
                "cwp://test/root",
                "test",
                Direction::Out,
                Window {
                    cursor: cursor.as_ref(),
                    limit: 2,
                    order,
                },
            )
            .unwrap();
            let mut refs: Vec<_> = page.items.into_iter().map(|e| e.reference).collect();
            if order == Order::Backward {
                refs.reverse();
            }
            seen.extend(refs);
            if !page.has_more {
                break;
            }
            cursor = page.next_cursor;
        }
        let mut expected: Vec<_> = all.items.iter().map(|e| e.reference.clone()).collect();
        if order == Order::Backward {
            expected.reverse();
        }
        assert_eq!(seen, expected);
    }
    let cursor = Cursor {
        scope: "cwp://test/wrong".into(),
        kind: "test".into(),
        direction: Direction::Out,
        position: 0,
        edge_ref: "test".into(),
    };
    assert!(queries::walk(
        &graph.connection,
        "cwp://test/root",
        "test",
        Direction::Out,
        Window {
            cursor: Some(&cursor),
            limit: 2,
            order: Order::Forward
        }
    )
    .is_err());
    assert!(queries::walk(
        &graph.connection,
        "cwp://test/root",
        "test",
        Direction::Out,
        Window::first(0)
    )
    .is_err());
}

#[test]
fn error_after_partial_message_write_rolls_everything_back() {
    let (_temp, mut graph) = fresh();
    demo::exercise(&mut graph).unwrap();
    graph
        .connection
        .execute_batch(
            "CREATE TRIGGER fail_shortcut BEFORE INSERT ON edges
        WHEN NEW.kind='message_in_workspace' BEGIN SELECT RAISE(ABORT,'injected failure'); END;",
        )
        .unwrap();
    let before: (i64, i64, i64) = graph
        .connection
        .query_row(
            "SELECT (SELECT count(*) FROM nodes),(SELECT count(*) FROM edges),(SELECT count(*) FROM ref_bindings)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let result = graph.append(&Append {
        message_ref: &reference("messages", 40),
        conversation: &reference("conversations", 0),
        author: &reference("actors", 0),
        text: "atomic",
        reply: None,
    });
    assert!(result.unwrap_err().to_string().contains("injected failure"));
    let after = graph
        .connection
        .query_row(
            "SELECT (SELECT count(*) FROM nodes),(SELECT count(*) FROM edges),(SELECT count(*) FROM ref_bindings)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn patches_preserve_unknown_values_and_reject_stale_writers() {
    let (_temp, mut graph) = fresh();
    graph
        .write(|tx| {
            insert_node(
                tx,
                "cwp://test/node",
                "future",
                &json!({"unknown": {"x": [1, null]}, "title": "before"}),
            )
        })
        .unwrap();
    graph
        .patch(
            "cwp://test/node",
            0,
            &json!({"title": "after", "empty": null}),
        )
        .unwrap();
    assert!(graph
        .patch("cwp://test/node", 0, &json!({"title": "lost update"}))
        .is_err());
    let n = node(&graph.connection, "cwp://test/node").unwrap();
    assert_eq!(
        n.props,
        json!({"unknown": {"x": [1, null]}, "title": "after", "empty": null})
    );
    graph
        .write(|tx| {
            tx.execute(
                "UPDATE nodes SET schema_id=?1 WHERE ref_id=?2",
                params![
                    bindings::bind(tx, &schema("future-2"))?,
                    bindings::resolve(tx, &n.reference)?
                ],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(require_kind(&graph.connection, "cwp://test/node", "future").is_err());
    assert!(node(&graph.connection, "cwp://test/node").is_ok());
}

#[test]
fn duplicate_edges_and_invalid_replies_do_not_create_records() {
    let (_temp, mut graph) = fresh();
    demo::exercise(&mut graph).unwrap();
    let input = Append {
        message_ref: &reference("messages", 0),
        conversation: &reference("conversations", 0),
        author: &reference("actors", 0),
        text: "duplicate",
        reply: None,
    };
    assert!(graph.append(&input).is_err());
    assert!(graph
        .write(|tx| {
            insert_edge(tx, input.message_ref, AUTHOR, input.author, 0, &json!({}))?;
            Ok(())
        })
        .is_err());
    graph
        .create_conversation(
            &reference("conversations", 1),
            &reference("workspaces", 0),
            "Other",
            1,
        )
        .unwrap();
    assert!(graph
        .append(&Append {
            message_ref: &reference("messages", 10),
            conversation: &reference("conversations", 1),
            reply: Some(input.message_ref),
            ..input
        })
        .is_err());
    assert!(node(&graph.connection, &reference("messages", 10)).is_err());
}

#[test]
fn simultaneous_writers_allocate_distinct_positions() {
    let (temp, mut graph) = fresh();
    demo::exercise(&mut graph).unwrap();
    let path = temp.path().join("graph.sqlite3");
    let other = Store::open(&path).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let joins: Vec<_> = [graph, other]
        .into_iter()
        .enumerate()
        .map(|(id, mut graph)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                graph
                    .append(&Append {
                        message_ref: &reference("messages", id + 10),
                        conversation: &reference("conversations", 0),
                        author: &reference("actors", 0),
                        text: "concurrent",
                        reply: None,
                    })
                    .unwrap();
            })
        })
        .collect();
    for join in joins {
        join.join().unwrap();
    }
    let graph = Store::open(&path).unwrap();
    let edges = queries::walk(
        &graph.connection,
        &reference("conversations", 0),
        CONTAINS,
        Direction::Out,
        Window::first(20),
    )
    .unwrap();
    assert_eq!(
        edges.items.iter().map(|e| e.position).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
}

#[test]
fn joined_timeline_preserves_tied_order_and_reply_fields_in_both_directions() {
    let (_temp, mut graph) = fresh();
    let workspace = reference("workspaces", 0);
    let conversation = reference("conversations", 0);
    let actor = reference("actors", 0);
    graph.create_workspace(&workspace).unwrap();
    graph.create_actor(&actor).unwrap();
    graph
        .create_conversation(&conversation, &workspace, "Paging", 0)
        .unwrap();
    for id in 0..9 {
        graph
            .append(&Append {
                message_ref: &reference("messages", id),
                conversation: &conversation,
                author: &actor,
                text: &format!("Message {id}"),
                reply: (id > 0).then(|| reference("messages", id - 1)).as_deref(),
            })
            .unwrap();
    }
    graph
        .connection
        .execute(
            "UPDATE edges SET position=position/3 WHERE kind=?1",
            [CONTAINS],
        )
        .unwrap();
    let edges = queries::walk(
        &graph.connection,
        &conversation,
        CONTAINS,
        Direction::Out,
        Window::first(50),
    )
    .unwrap();
    for order in [Order::Forward, Order::Backward] {
        let mut cursor = None;
        let mut seen = Vec::new();
        loop {
            let page = queries::timeline(
                &graph.connection,
                &conversation,
                Window {
                    cursor: cursor.as_ref(),
                    limit: 2,
                    order,
                },
            )
            .unwrap();
            for message in &page.items {
                let id: usize = message
                    .text
                    .strip_prefix("Message ")
                    .unwrap()
                    .parse()
                    .unwrap();
                assert_eq!(message.author_actor_ref, actor);
                assert_eq!(
                    message.reply_to_message_ref,
                    (id > 0).then(|| reference("messages", id - 1))
                );
            }
            let mut refs: Vec<_> = page.items.into_iter().map(|m| m.message_ref).collect();
            if order == Order::Backward {
                refs.reverse();
            }
            seen.extend(refs);
            if !page.has_more {
                break;
            }
            cursor = page.next_cursor;
        }
        let mut expected: Vec<_> = edges.items.iter().map(|e| e.target_ref.clone()).collect();
        if order == Order::Backward {
            expected.reverse();
        }
        assert_eq!(seen, expected);
    }
    graph
        .write(|tx| {
            tx.execute(
                "UPDATE edges SET schema_id=?1 WHERE kind=?2",
                params![bindings::bind(tx, &schema("future-edge"))?, AUTHOR],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(queries::timeline(&graph.connection, &conversation, Window::first(50)).is_err());
}

#[test]
fn generic_indexes_support_forward_reverse_and_ordered_reads() {
    let (_temp, graph) = fresh();
    let plans = queries::plans(&graph.connection).unwrap();
    assert!(plans["forward"]
        .to_string()
        .contains("COVERING INDEX edges_out"));
    assert!(plans["reverse"]
        .to_string()
        .contains("COVERING INDEX edges_in"));
    assert!(plans["ordered"]
        .to_string()
        .contains("COVERING INDEX edges_out_ordered"));
    assert!(!plans.to_string().contains("TEMP B-TREE"));
}

#[test]
fn unrelated_database_is_refused_without_rewriting_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("other.sqlite3");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE original(value TEXT); INSERT INTO original VALUES ('keep');")
        .unwrap();
    assert!(Store::open(&path).is_err());
    assert_eq!(
        db.query_row("SELECT value FROM original", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "keep"
    );
}

#[test]
fn failed_moves_and_rebuilds_preserve_canonical_and_derived_membership() {
    let (_temp, mut graph) = fresh();
    demo::exercise(&mut graph).unwrap();
    let conversation = reference("conversations", 0);
    let source = reference("workspaces", 1);
    let destination = reference("workspaces", 0);
    graph.archive(&conversation, true).unwrap();
    graph
        .connection
        .execute_batch(
            "CREATE TRIGGER fail_move BEFORE UPDATE ON edges
        WHEN NEW.kind='message_in_workspace' BEGIN SELECT RAISE(ABORT,'move failed'); END;",
        )
        .unwrap();
    assert!(graph
        .move_conversation(&conversation, &destination)
        .is_err());
    assert_eq!(
        target(&graph.connection, &conversation, WORKSPACE).unwrap(),
        source
    );
    assert_eq!(
        queries::conversations(&graph.connection, &source, true).unwrap(),
        vec![conversation.clone()]
    );
    assert!(
        queries::conversations(&graph.connection, &destination, true)
            .unwrap()
            .is_empty()
    );
    graph
        .connection
        .execute_batch("DROP TRIGGER fail_move")
        .unwrap();
    graph
        .move_conversation(&conversation, &destination)
        .unwrap();
    assert_eq!(
        queries::conversations(&graph.connection, &destination, true).unwrap(),
        vec![conversation]
    );
    assert!(queries::conversations(&graph.connection, &source, true)
        .unwrap()
        .is_empty());
    let before = queries::walk(
        &graph.connection,
        &destination,
        SHORTCUT,
        Direction::In,
        Window::first(10),
    )
    .unwrap();
    assert_eq!(before.items.len(), 3);
    graph
        .connection
        .execute_batch(
            "CREATE TRIGGER fail_rebuild BEFORE INSERT ON edges
        WHEN NEW.kind='message_in_workspace' BEGIN SELECT RAISE(ABORT,'rebuild failed'); END;",
        )
        .unwrap();
    assert!(graph.rebuild_shortcuts().is_err());
    let after = queries::walk(
        &graph.connection,
        &destination,
        SHORTCUT,
        Direction::In,
        Window::first(10),
    )
    .unwrap();
    assert_eq!(before.items, after.items);
}
