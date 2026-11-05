use std::{path::Path, time::Duration};

use rusqlite::{params, Connection, TransactionBehavior};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    bindings::{self, RefId},
    model::*,
};

pub struct Store {
    pub connection: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        // Apply the immutable-binding trigger to REPLACE's implicit deletion too.
        connection.pragma_update(None, "recursive_triggers", "ON")?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let tables: i64 = tx.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        if tables == 0 {
            tx.execute_batch(include_str!("schema.sql"))?;
            insert_node(&tx, STORE_REF, "store", &json!({"name": "cwpDb spike"}))?;
        } else {
            let stored = node(&tx, STORE_REF)
                .map_err(|_| "not a cwpDb spike database; choose a fresh path")?;
            ensure(
                stored.schema_ref == schema("store"),
                "unsupported cwpDb store schema",
            )?;
        }
        tx.commit()?;
        let mode: String = connection.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        ensure(mode == "wal", "WAL mode is required")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        Ok(Self { connection })
    }

    pub fn write<T>(&mut self, operation: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = operation(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn patch(
        &mut self,
        reference: &str,
        expected_revision: i64,
        updates: &Value,
    ) -> Result<()> {
        ensure(updates.is_object(), "property updates must be an object")?;
        self.write(|tx| {
            let mut stored = node(tx, reference)?;
            ensure(
                stored.revision == expected_revision,
                "stale record revision",
            )?;
            // Shallow assignment preserves unmentioned and unfamiliar properties, including nulls.
            let props = stored
                .props
                .as_object_mut()
                .ok_or("node properties must be an object")?;
            for (key, value) in updates.as_object().ok_or("updates must be an object")? {
                props.insert(key.clone(), value.clone());
            }
            tx.execute(
                "UPDATE nodes SET props=jsonb(?1), revision=revision+1 WHERE ref_id=?2",
                params![stored.props.to_string(), bindings::resolve(tx, reference)?],
            )?;
            Ok(())
        })
    }

    pub fn statistics(&self) -> Result<Value> {
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        let nodes: i64 = self
            .connection
            .query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))?;
        let edges: i64 = self
            .connection
            .query_row("SELECT count(*) FROM edges", [], |r| r.get(0))?;
        let bindings: i64 =
            self.connection
                .query_row("SELECT count(*) FROM ref_bindings", [], |r| r.get(0))?;
        let pages: i64 = self
            .connection
            .query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let size: i64 = self
            .connection
            .query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let objects = self.connection.prepare(
            "SELECT type, name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
        )?.query_map([], |r| Ok(json!({"type": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?})))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(
            json!({"nodes": nodes, "edges": edges, "ref_bindings": bindings, "database_bytes": pages*size,
            "schema_objects": objects, "sqlite_version": rusqlite::version(),
            "journal_mode": "wal", "synchronous": "FULL", "schema_ref": schema("store")}),
        )
    }
}

pub fn node(db: &Connection, reference: &str) -> Result<Node> {
    let (kind, schema_ref, revision, props): (String, String, i64, String) = db.query_row(
        "SELECT n.kind,s.canonical_ref,n.revision,json(n.props) FROM nodes n
        JOIN ref_bindings s ON s.ref_id=n.schema_id
        WHERE n.ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",
        [reference],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    Ok(Node {
        reference: reference.into(),
        kind,
        schema_ref,
        revision,
        props: serde_json::from_str(&props)?,
    })
}

pub fn require_kind(db: &Connection, reference: &str, kind: &str) -> Result<RefId> {
    let (id, stored_kind, stored_schema): (RefId, String, String) = db
        .prepare_cached(
            "SELECT n.ref_id,n.kind,s.canonical_ref FROM nodes n
        JOIN ref_bindings s ON s.ref_id=n.schema_id
        WHERE n.ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",
        )?
        .query_row([reference], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    ensure(
        stored_kind == kind && stored_schema == schema(kind),
        "unsupported node kind or schema",
    )?;
    Ok(id)
}

pub fn insert_node(db: &Connection, reference: &str, kind: &str, props: &Value) -> Result<RefId> {
    ensure(props.is_object(), "node properties must be objects")?;
    let id = bindings::bind(db, reference)?;
    let schema_id = bindings::bind(db, &schema(kind))?;
    db.prepare_cached(
        "INSERT INTO nodes(ref_id,kind,schema_id,props) VALUES (?1,?2,?3,jsonb(?4))",
    )?
    .execute(params![id, kind, schema_id, props.to_string()])?;
    Ok(id)
}

pub fn insert_edge(
    db: &Connection,
    source: &str,
    kind: &str,
    target: &str,
    position: i64,
    props: &Value,
) -> Result<String> {
    let source_id = bindings::resolve(db, source)?;
    let target_id = bindings::resolve(db, target)?;
    insert_edge_ids(db, source_id, kind, target_id, position, props)
}

pub fn insert_edge_ids(
    db: &Connection,
    source: RefId,
    kind: &str,
    target: RefId,
    position: i64,
    props: &Value,
) -> Result<String> {
    let reference = format!("{ROOT}/edges/{}", Uuid::new_v4().simple());
    let id = bindings::bind(db, &reference)?;
    let schema_id = bindings::bind(db, EDGE_SCHEMA)?;
    db.prepare_cached(
        "INSERT INTO edges(ref_id,kind,schema_id,source_id,target_id,position,props) VALUES (?1,?2,?3,?4,?5,?6,jsonb(?7))",
    )?.execute(params![id, kind, schema_id, source, target, position, props.to_string()])?;
    Ok(reference)
}

pub fn target(db: &Connection, source: &str, kind: &str) -> Result<String> {
    bindings::expand(db, target_id(db, bindings::resolve(db, source)?, kind)?)
}

pub fn target_id(db: &Connection, source: RefId, kind: &str) -> Result<RefId> {
    Ok(db
        .prepare_cached("SELECT target_id FROM edges WHERE source_id=?1 AND kind=?2")?
        .query_row(params![source, kind], |r| r.get(0))?)
}

pub fn edge_exists(db: &Connection, source: RefId, kind: &str, target: RefId) -> Result<bool> {
    Ok(db
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=?1 AND kind=?2 AND target_id=?3)",
        )?
        .query_row(params![source, kind, target], |r| r.get(0))?)
}
