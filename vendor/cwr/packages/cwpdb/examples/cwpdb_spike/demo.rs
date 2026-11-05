use std::path::Path;

use rusqlite::params;
use serde_json::{json, Value};

use super::{model::*, queries, store::*};

pub fn run(directory: &Path) -> Result<Value> {
    std::fs::create_dir(directory)?;
    let path = directory.join("graph.sqlite3");
    let mut store = Store::open(&path)?;
    let result = exercise(&mut store)?;
    drop(store);
    let reopened = Store::open(&path)?;
    let conversation = reference("conversations", 0);
    let restored = queries::timeline(&reopened.connection, &conversation, Window::first(10))?;
    ensure(restored.items.len() == 3, "restart lost messages")?;
    let result = json!({"database": path, "workflow": result, "reopen_messages": restored.items,
        "statistics": reopened.statistics()?, "query_plans": queries::plans(&reopened.connection)?});
    std::fs::write(
        directory.join("demo.json"),
        serde_json::to_string_pretty(&result)?,
    )?;
    Ok(result)
}

pub fn exercise(store: &mut Store) -> Result<Value> {
    let workspace = reference("workspaces", 0);
    let other = reference("workspaces", 1);
    let actor = reference("actors", 0);
    let conversation = reference("conversations", 0);
    store.create_workspace(&workspace)?;
    store.create_workspace(&other)?;
    store.create_actor(&actor)?;
    store.create_conversation(&conversation, &workspace, "cwpDb conversation", 0)?;
    for i in 0..3 {
        store.append(&Append {
            message_ref: &reference("messages", i),
            conversation: &conversation,
            author: &actor,
            text: &format!("Message {} — stored as JSONB", i + 1),
            reply: (i > 0).then(|| reference("messages", i - 1)).as_deref(),
        })?;
    }
    let first = queries::timeline(&store.connection, &conversation, Window::first(2))?;
    let second = queries::timeline(
        &store.connection,
        &conversation,
        Window {
            cursor: first.next_cursor.as_ref(),
            limit: 2,
            order: Order::Forward,
        },
    )?;
    ensure(
        first.items.len() == 2 && first.has_more && second.items.len() == 1 && !second.has_more,
        "pagination failed",
    )?;
    let author_edges = queries::walk(
        &store.connection,
        &actor,
        AUTHOR,
        Direction::In,
        Window::first(10),
    )?;
    ensure(author_edges.items.len() == 3, "reverse traversal failed")?;

    store.archive(&conversation, true)?;
    ensure(
        queries::conversations(&store.connection, &workspace, false)?.is_empty(),
        "archived conversation is active",
    )?;
    let archived = queries::conversations(&store.connection, &workspace, true)?;
    store.archive(&conversation, false)?;
    ensure(
        queries::conversations(&store.connection, &workspace, false)? == vec![conversation.clone()],
        "restore failed",
    )?;

    store.patch(
        &conversation,
        0,
        &json!({"future_property": {"keep": true}}),
    )?;
    store.patch(
        &conversation,
        1,
        &json!({"title": "Renamed without a migration"}),
    )?;
    ensure(
        node(&store.connection, &conversation)?.props["future_property"]["keep"] == true,
        "unknown property was lost",
    )?;

    let absent = reference("actors", 999);
    let partial = reference("messages", 999);
    let failed = store.write(|tx| {
        insert_node(tx, &partial, "message", &json!({"text": "must roll back"}))?;
        insert_edge(tx, &partial, AUTHOR, &absent, 0, &json!({}))?;
        Ok(())
    });
    ensure(
        failed.is_err() && node(&store.connection, &partial).is_err(),
        "failed write did not roll back",
    )?;

    store.move_conversation(&conversation, &other)?;
    ensure(
        queries::walk(
            &store.connection,
            &workspace,
            SHORTCUT,
            Direction::In,
            Window::first(10),
        )?
        .items
        .is_empty(),
        "stale shortcut after move",
    )?;
    ensure(
        queries::walk(
            &store.connection,
            &other,
            SHORTCUT,
            Direction::In,
            Window::first(10),
        )?
        .items
        .len()
            == 3,
        "shortcut move failed",
    )?;
    store
        .connection
        .execute("DELETE FROM edges WHERE kind=?1", params![SHORTCUT])?;
    ensure(store.rebuild_shortcuts()? == 3, "shortcut rebuild failed")?;
    ensure(
        queries::walk(
            &store.connection,
            &other,
            SHORTCUT,
            Direction::In,
            Window::first(10),
        )?
        .items
        .len()
            == 3,
        "rebuilt shortcuts are missing",
    )?;
    Ok(
        json!({"checks": ["forward_and_reverse_traversal", "timeline_pagination", "archive_restore",
        "unknown_property_preservation", "failed_write_rollback", "atomic_shortcut_move", "shortcut_rebuild"],
        "conversation": conversation, "workspace": other, "actor": actor,
        "first_page": first, "second_page": second, "archived_conversations": archived}),
    )
}
