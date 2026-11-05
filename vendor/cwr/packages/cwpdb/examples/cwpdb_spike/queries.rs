use rusqlite::{params, Connection};

use super::{
    bindings::{self, RefId},
    model::*,
};

pub fn walk(
    db: &Connection,
    scope: &str,
    kind: &str,
    direction: Direction,
    window: Window<'_>,
) -> Result<Page<Edge>> {
    validate_window(scope, kind, direction, &window)?;
    let (scope_id, cursor_id) = coordinates(db, scope, &window)?;
    let endpoint = if direction == Direction::Out {
        "source_id"
    } else {
        "target_id"
    };
    let (comparison, order) = if window.order == Order::Forward {
        (">", "ASC")
    } else {
        ("<", "DESC")
    };
    let mut sql = format!("SELECT r.canonical_ref,e.kind,s.canonical_ref,src.canonical_ref,dst.canonical_ref,e.position,json(e.props)
        FROM edges e JOIN ref_bindings r ON r.ref_id=e.ref_id
        JOIN ref_bindings s ON s.ref_id=e.schema_id
        JOIN ref_bindings src ON src.ref_id=e.source_id
        JOIN ref_bindings dst ON dst.ref_id=e.target_id
        WHERE e.{endpoint}=?1 AND e.kind=?2");
    if window.cursor.is_some() {
        sql.push_str(&format!(" AND (e.position,e.ref_id) {comparison} (?4,?5)"));
    }
    sql.push_str(&format!(
        " ORDER BY e.position {order},e.ref_id {order} LIMIT ?3"
    ));
    let mut query = db.prepare_cached(&sql)?;
    let take = i64::try_from(window.limit + 1)?;
    let mut rows = if let Some(cursor) = window.cursor {
        query.query(params![scope_id, kind, take, cursor.position, cursor_id])?
    } else {
        query.query(params![scope_id, kind, take])?
    };
    let mut edges = Vec::new();
    while let Some(row) = rows.next()? {
        let props: String = row.get(6)?;
        edges.push(Edge {
            reference: row.get(0)?,
            kind: row.get(1)?,
            schema_ref: row.get(2)?,
            source_ref: row.get(3)?,
            target_ref: row.get(4)?,
            position: row.get(5)?,
            props: serde_json::from_str(&props)?,
        });
    }
    let has_more = edges.len() > window.limit;
    edges.truncate(window.limit);
    let next_cursor = edges.last().filter(|_| has_more).map(|edge| Cursor {
        scope: scope.into(),
        kind: kind.into(),
        direction,
        position: edge.position,
        edge_ref: edge.reference.clone(),
    });
    if window.order == Order::Backward {
        edges.reverse();
    }
    Ok(Page {
        items: edges,
        has_more,
        next_cursor,
    })
}

// Resolve immutable input bindings, then hydrate the page and output URLs in one query.
pub fn timeline(db: &Connection, conversation: &str, window: Window<'_>) -> Result<Page<Message>> {
    validate_window(conversation, CONTAINS, Direction::Out, &window)?;
    let (conversation_id, cursor_id) = coordinates(db, conversation, &window)?;
    let message_schema = bindings::find(db, &schema("message"))?;
    let edge_schema = bindings::find(db, EDGE_SCHEMA)?;
    let sql = timeline_sql(&window);
    let mut query = db.prepare_cached(&sql)?;
    let mut rows = query.query(params![
        conversation_id,
        CONTAINS,
        i64::try_from(window.limit + 1)?,
        window.cursor.map_or(0, |c| c.position),
        cursor_id,
        AUTHOR,
        REPLY
    ])?;
    let mut items = Vec::new();
    let mut keys = Vec::new();
    while let Some(row) = rows.next()? {
        ensure(
            row.get::<_, String>(2)? == "message"
                && Some(row.get::<_, RefId>(3)?) == message_schema,
            "unsupported message schema",
        )?;
        ensure(
            Some(row.get::<_, RefId>(8)?) == edge_schema
                && row.get::<_, Option<RefId>>(9)? == edge_schema
                && row
                    .get::<_, Option<RefId>>(10)?
                    .is_none_or(|id| Some(id) == edge_schema),
            "unsupported relationship schema or missing author",
        )?;
        keys.push((row.get::<_, i64>(1)?, row.get::<_, String>(0)?));
        items.push(Message {
            message_ref: row.get(4)?,
            text: row.get(5)?,
            author_actor_ref: row.get(6)?,
            reply_to_message_ref: row.get(7)?,
        });
    }
    let has_more = items.len() > window.limit;
    items.truncate(window.limit);
    keys.truncate(window.limit);
    let next_cursor = keys
        .last()
        .filter(|_| has_more)
        .map(|(position, edge_ref)| Cursor {
            scope: conversation.into(),
            kind: CONTAINS.into(),
            direction: Direction::Out,
            position: *position,
            edge_ref: edge_ref.clone(),
        });
    if window.order == Order::Backward {
        items.reverse();
    }
    Ok(Page {
        items,
        has_more,
        next_cursor,
    })
}

fn validate_window(
    scope: &str,
    kind: &str,
    direction: Direction,
    window: &Window<'_>,
) -> Result<()> {
    ensure(
        (1..=500).contains(&window.limit),
        "page size must be between 1 and 500",
    )?;
    if let Some(cursor) = window.cursor {
        ensure(
            cursor.scope == scope && cursor.kind == kind && cursor.direction == direction,
            "cursor belongs to another traversal",
        )?;
    }
    Ok(())
}

fn coordinates(
    db: &Connection,
    scope: &str,
    window: &Window<'_>,
) -> Result<(Option<RefId>, RefId)> {
    let scope_id = bindings::find(db, scope)?;
    let cursor_id = window
        .cursor
        .map(|c| bindings::resolve(db, &c.edge_ref))
        .transpose()?
        .unwrap_or(0);
    Ok((scope_id, cursor_id))
}

fn timeline_sql(window: &Window<'_>) -> String {
    let (comparison, order) = if window.order == Order::Forward {
        (">", "ASC")
    } else {
        ("<", "DESC")
    };
    let mut sql = String::from("SELECT er.canonical_ref,e.position,n.kind,n.schema_id,nr.canonical_ref,json_extract(n.props,'$.text'),ar.canonical_ref,rr.canonical_ref,e.schema_id,a.schema_id,r.schema_id
        FROM edges e JOIN nodes n ON n.ref_id=e.target_id
        JOIN ref_bindings er ON er.ref_id=e.ref_id
        JOIN ref_bindings nr ON nr.ref_id=n.ref_id
        LEFT JOIN edges a ON a.source_id=n.ref_id AND a.kind=?6
        LEFT JOIN edges r ON r.source_id=n.ref_id AND r.kind=?7
        LEFT JOIN ref_bindings ar ON ar.ref_id=a.target_id
        LEFT JOIN ref_bindings rr ON rr.ref_id=r.target_id
        WHERE e.source_id=?1 AND e.kind=?2");
    if window.cursor.is_some() {
        sql.push_str(&format!(" AND (e.position,e.ref_id) {comparison} (?4,?5)"));
    }
    sql.push_str(&format!(
        " ORDER BY e.position {order},e.ref_id {order} LIMIT ?3"
    ));
    sql
}

pub fn conversations(db: &Connection, workspace: &str, archived: bool) -> Result<Vec<String>> {
    let predicate = if archived { "EXISTS" } else { "NOT EXISTS" };
    let sql = format!(
        "SELECT b.canonical_ref FROM edges e JOIN ref_bindings b ON b.ref_id=e.source_id
        WHERE e.target_id=?1 AND e.kind=?2 AND {predicate} (
        SELECT 1 FROM edges a WHERE a.source_id=e.source_id AND a.kind=?3 AND a.target_id=?4
    ) ORDER BY e.position,e.ref_id"
    );
    Ok(db
        .prepare_cached(&sql)?
        .query_map(
            params![
                bindings::find(db, workspace)?,
                WORKSPACE,
                COLLECTION,
                bindings::find(db, &archive_ref(workspace))?
            ],
            |r| r.get(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn explain(db: &Connection, sql: &str, args: impl rusqlite::Params) -> Result<Vec<String>> {
    Ok(db
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
        .query_map(args, |r| r.get(3))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn plans(db: &Connection) -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "forward": explain(db, "SELECT target_id FROM edges WHERE source_id=?1 AND kind=?2", params![0, CONTAINS])?,
        "reverse": explain(db, "SELECT source_id FROM edges WHERE target_id=?1 AND kind=?2", params![0, AUTHOR])?,
        "ordered": explain(db, "SELECT ref_id,target_id,position FROM edges WHERE source_id=?1 AND kind=?2 AND (position,ref_id)>(?3,?4) ORDER BY position,ref_id LIMIT 51", params![0, CONTAINS, 0, 0])?,
        "timeline": explain(db, &timeline_sql(&Window::first(50)), params![0, CONTAINS, 51, 0, 0, AUTHOR, REPLY])?,
        "reference_lookup": explain(db, "SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1", [STORE_REF])?
    }))
}
