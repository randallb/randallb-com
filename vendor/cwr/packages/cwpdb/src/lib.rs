//! SQLite property-graph primitives and a small JSON-native storage boundary.
//!
//! Integer bindings are private to one database; canonical CWP URLs identify
//! records and schemas across databases. Graph callers own their transactions.

#[cfg(feature = "test-support")]
pub mod benchmark;
pub mod documents;
pub mod environment;
pub mod invocation;
pub mod metrics;
pub mod profiling;
pub mod remote;
#[cfg(feature = "test-support")]
pub mod test_support;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;
use std::collections::BTreeSet;

pub mod disposable;
mod ordered_membership;
pub mod sequence_read;
pub mod sequence_receipt;
pub use ordered_membership::{read_ordered_members, replace_ordered_members, OrderedMember};
pub mod edge_read;
mod identity;
pub mod legacy_workspace;
mod native;
pub mod ref_read;
pub mod unordered;

use identity::claim_record_identity;
pub use identity::{claim_identity, derived_ref, identity_claim, record_exists, IdentityClaim};

pub use native::{
    insert_entity, replace_entity, BatchRequest, BatchResponse, DiagnosticPage, DiagnosticRecord,
    FfiStore, GraphBatchRequest, GraphOperation, NativeStore, QueryRequest, QueryResponse,
    SqlStatement, WalCheckpoint,
};

/// Error type shared by graph and native-store callers.
pub type CwpdbResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A database-local integer identity for a canonical CWP reference.
pub type RefId = i64;

/// Whether a graph identity remains after its record is removed.
///
/// Canonical application references are durable by default. Some local,
/// disposable data such as performance observations needs graph identities for
/// indexed relationships without permanently growing that dictionary. Those
/// callers must opt into [`BindingRetention::Disposable`] and later reclaim the
/// whole graph record through [`remove_disposable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingRetention {
    /// The canonical reference is immutable and remains bound forever.
    Durable,
    /// The local graph reference may be reclaimed once no graph record uses it.
    Disposable,
}

/// The schema URL written by CWR to identify the shared physical graph layout.
pub const SCHEMA_REF: &str = "cwp://org.coworkerprotocol/cwpdb/schemas/store/2026-09";

/// The schema URL for generic graph relationships.
pub const EDGE_SCHEMA_REF: &str = "cwp://org.coworkerprotocol/cwpdb/schemas/edge/2026-09";

mod physical_schema;
pub use physical_schema::{initialize, schema_is_current, upgrade, PHYSICAL_SCHEMA_SQL};

/// Look up a canonical reference's database-local identity.
pub fn find(db: &Connection, reference: &str) -> CwpdbResult<Option<RefId>> {
    Ok(db
        .prepare_cached("SELECT ref_id FROM ref_bindings WHERE canonical_ref = ?1")?
        .query_row([reference], |row| row.get(0))
        .optional()?)
}

/// Bind a canonical CWP reference to a database-local identity.
pub fn bind(db: &Transaction<'_>, reference: &str) -> CwpdbResult<RefId> {
    bind_with_retention(db, reference, BindingRetention::Durable)
}

/// Bind a local graph reference whose identity can be reclaimed with its
/// expiring graph record.
pub fn bind_disposable(db: &Transaction<'_>, reference: &str) -> CwpdbResult<RefId> {
    bind_with_retention(db, reference, BindingRetention::Disposable)
}

fn bind_with_retention(
    db: &Transaction<'_>,
    reference: &str,
    retention: BindingRetention,
) -> CwpdbResult<RefId> {
    if let Some(id) = find(db, reference)? {
        // A normal application record can later claim a local identity. That
        // promotion is safe; the reverse would allow an expiring caller to
        // reclaim a durable application reference and is rejected by schema.
        if retention == BindingRetention::Durable {
            db.execute(
                "UPDATE ref_bindings SET disposable=0 WHERE ref_id=?1 AND disposable=1",
                [id],
            )?;
        }
        return Ok(id);
    }
    Ok(db
        .prepare_cached(
            "INSERT INTO ref_bindings(canonical_ref,disposable) VALUES (?1,?2) RETURNING ref_id",
        )?
        .query_row(
            params![
                reference,
                if matches!(retention, BindingRetention::Disposable) {
                    1
                } else {
                    0
                }
            ],
            |row| row.get(0),
        )?)
}

/// Return the current graph-schema URL for a node kind.
pub fn schema_ref(kind: &str) -> String {
    format!("cwp://org.coworkerprotocol/cwpdb/schemas/{kind}/2026-09")
}

/// Merge properties into a node, replacing its `record` property as a whole.
///
/// Unmentioned outer properties survive through JSON Merge Patch. A node cannot
/// silently change kind or schema after its first write.
pub fn put_node(
    db: &Transaction<'_>,
    reference: &str,
    kind: &str,
    props: &Value,
) -> CwpdbResult<RefId> {
    put_node_with_schema(db, reference, kind, &schema_ref(kind), props)
}

/// Create a graph node whose reference is reclaimed when the caller removes
/// the expiring graph record with [`remove_disposable`].
pub fn put_disposable_node(
    db: &Transaction<'_>,
    reference: &str,
    kind: &str,
    props: &Value,
) -> CwpdbResult<RefId> {
    put_node_with_retention(
        db,
        reference,
        kind,
        &schema_ref(kind),
        props,
        BindingRetention::Disposable,
    )
}

/// Store a node using a product-owned schema reference.
pub fn put_node_with_schema(
    db: &Transaction<'_>,
    reference: &str,
    kind: &str,
    schema_reference: &str,
    props: &Value,
) -> CwpdbResult<RefId> {
    put_node_with_retention(
        db,
        reference,
        kind,
        schema_reference,
        props,
        BindingRetention::Durable,
    )
}

fn put_node_with_retention(
    db: &Transaction<'_>,
    reference: &str,
    kind: &str,
    schema_reference: &str,
    props: &Value,
    retention: BindingRetention,
) -> CwpdbResult<RefId> {
    let id = claim_record_identity(db, reference, &format!("node:{kind}"), retention)?;
    let schema = bind(db, schema_reference)?;
    let changed = db.prepare_cached(
        "INSERT INTO nodes(ref_id,kind,schema_id,props) VALUES (?1,?2,?3,jsonb(?4))
         ON CONFLICT(ref_id) DO UPDATE SET props=jsonb_patch(CASE WHEN json_type(excluded.props,'$.record') IS NOT NULL THEN jsonb_remove(nodes.props,'$.record') ELSE nodes.props END,excluded.props), revision=nodes.revision+1
         WHERE nodes.kind=excluded.kind AND nodes.schema_id=excluded.schema_id",
    )?.execute(params![id, kind, schema, serde_json::to_string(props)?])?;
    if changed != 1 {
        return Err(format!("unsupported node kind or schema for {reference}").into());
    }
    Ok(id)
}

/// Read a node's JSON properties by canonical reference.
pub fn node_props(db: &Connection, reference: &str) -> CwpdbResult<Option<Value>> {
    let json: Option<String> = db
        .prepare_cached(
            "SELECT json(props) FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",
        )?
        .query_row([reference], |row| row.get(0))
        .optional()?;
    json.map(|json| serde_json::from_str(&json).map_err(Into::into))
        .transpose()
}

/// Merge properties into one directed relationship.
pub fn put_edge(
    db: &Transaction<'_>,
    source: &str,
    kind: &str,
    target: &str,
    position: i64,
    props: &Value,
) -> CwpdbResult<RefId> {
    put_edge_with_retention(
        db,
        source,
        kind,
        target,
        position,
        props,
        BindingRetention::Durable,
    )
}

/// Create a graph relationship whose generated identity can be reclaimed with
/// its disposable endpoints.
pub fn put_disposable_edge(
    db: &Transaction<'_>,
    source: &str,
    kind: &str,
    target: &str,
    position: i64,
    props: &Value,
) -> CwpdbResult<RefId> {
    put_edge_with_retention(
        db,
        source,
        kind,
        target,
        position,
        props,
        BindingRetention::Disposable,
    )
}

fn put_edge_with_retention(
    db: &Transaction<'_>,
    source: &str,
    kind: &str,
    target: &str,
    position: i64,
    props: &Value,
    retention: BindingRetention,
) -> CwpdbResult<RefId> {
    let source_id = bind_with_retention(db, source, retention)?;
    let target_id = bind_with_retention(db, target, retention)?;
    let reference = derived_ref(kind, &[source, target]);
    let id = claim_record_identity(db, &reference, &format!("edge:{kind}"), retention)?;
    let schema = bind(db, EDGE_SCHEMA_REF)?;
    // A conflicting schema or identity yields NULL and fails the NOT NULL
    // constraint. Unchanged compatible edges avoid a write.
    db.prepare_cached(
        "INSERT INTO edges(ref_id,kind,schema_id,source_id,target_id,position,props) VALUES (?1,?2,?3,?4,?5,?6,jsonb(?7))
         ON CONFLICT(source_id,kind,target_id) DO UPDATE SET position=excluded.position,props=CASE WHEN edges.schema_id=excluded.schema_id AND edges.ref_id=excluded.ref_id THEN jsonb_patch(CASE WHEN json_type(excluded.props,'$.record') IS NOT NULL THEN jsonb_remove(edges.props,'$.record') ELSE edges.props END,excluded.props) END,revision=edges.revision+1
         WHERE edges.schema_id!=excluded.schema_id OR edges.ref_id!=excluded.ref_id OR edges.position!=excluded.position OR edges.props!=jsonb_patch(CASE WHEN json_type(excluded.props,'$.record') IS NOT NULL THEN jsonb_remove(edges.props,'$.record') ELSE edges.props END,excluded.props)",
    )?.execute(params![id, kind, schema, source_id, target_id, position, serde_json::to_string(props)?])?;
    Ok(id)
}

/// Return a source's relationship targets in their stable position order.
pub fn targets(db: &Connection, source: &str, kind: &str) -> CwpdbResult<Vec<String>> {
    Ok(db
        .prepare_cached(
            "SELECT r.canonical_ref FROM edges e JOIN ref_bindings r ON r.ref_id=e.target_id
             WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind=?2 ORDER BY e.position,e.ref_id",
        )?
        .query_map(params![source, kind], |row| row.get(0))?
        .collect::<Result<_, _>>()?)
}

/// Remove a record and relationships referring to it, including indexes of
/// relationships. Its immutable reference binding remains in the graph.
pub fn remove(db: &Transaction<'_>, reference: &str) -> CwpdbResult<()> {
    if let Some(id) = find(db, reference)? {
        // Runtime receipts can cover a large CWR atomic mutation. Their direct
        // dependency index expires the shared receipt payload when any protected
        // input record is removed, without a long per-record receipt edge chain.
        let payloads=db.prepare_cached("SELECT DISTINCT dependency.target_id FROM edges dependency JOIN edges payload ON payload.ref_id=dependency.target_id WHERE dependency.source_id=?1 AND dependency.kind='cwpdb:runtime_receipt_dependency_v1' AND payload.kind='sequence_mutation_receipt_v1'")?.query_map([id],|row|row.get::<_,RefId>(0))?.collect::<Result<Vec<_>,_>>()?;
        for payload in payloads {
            let candidates = remove_by_id(db, payload)?;
            reclaim_disposable_bindings_for(db, candidates)?;
        }
        let _ = remove_by_id(db, id)?;
    }
    Ok(())
}

/// Remove a disposable graph record and reclaim disposable identities that no
/// remaining node or relationship uses.
///
/// Durable canonical references remain protected by the graph schema. This is
/// deliberately a separate operation from [`remove`], which preserves the
/// historical binding for application records.
pub fn remove_disposable(db: &Transaction<'_>, reference: &str) -> CwpdbResult<()> {
    remove_disposables(db, [reference])
}

/// Remove several disposable graph records and reclaim their now-unused
/// identities together.
///
/// Retention callers should prefer this over repeated [`remove_disposable`]
/// calls. It validates every requested reference before changing the graph,
/// then examines only identities touched by the removed subtrees. That keeps a
/// large expired batch from rescanning unrelated observations.
pub fn remove_disposables<'a>(
    db: &Transaction<'_>,
    references: impl IntoIterator<Item = &'a str>,
) -> CwpdbResult<()> {
    let references = references.into_iter().collect::<Vec<_>>();
    let mut records = Vec::with_capacity(references.len());
    for reference in &references {
        let Some(id) = find(db, reference)? else {
            continue;
        };
        let disposable: i64 = db.query_row(
            "SELECT disposable FROM ref_bindings WHERE ref_id=?1",
            [id],
            |row| row.get(0),
        )?;
        if disposable == 0 {
            return Err(format!("{reference} has a durable graph identity").into());
        }
        // A retained edge may index another edge. Inspect the complete doomed
        // subtree before any deletion, so retention cannot erase durable work.
        let touches_durable: bool = db.query_row(
            "WITH RECURSIVE doomed(id) AS (
                SELECT ?1 UNION
                SELECT e.ref_id FROM edges e JOIN doomed d ON e.source_id=d.id OR e.target_id=d.id
            ) SELECT EXISTS(SELECT 1 FROM doomed d JOIN ref_bindings r ON r.ref_id=d.id WHERE r.disposable=0)",
            [id], |row| row.get(0),
        )?;
        if touches_durable {
            return Err(format!("{reference} is referenced by a durable graph record").into());
        }
        records.push(id);
    }

    let mut candidates = BTreeSet::new();
    for id in records {
        candidates.extend(remove_by_id(db, id)?);
    }
    reclaim_disposable_bindings_for(db, candidates)
}

/// Reclaim disposable bindings that no longer occur in the graph.
///
/// This is useful after a domain removes an expiring graph subtree. It never
/// deletes a durable binding or a remaining graph record; it only releases
/// disposable identities that no node or relationship still uses.
pub fn reclaim_disposable_bindings(db: &Transaction<'_>) -> CwpdbResult<()> {
    let candidates = db
        .prepare_cached("SELECT ref_id FROM ref_bindings WHERE disposable=1")?
        .query_map([], |row| row.get::<_, RefId>(0))?
        .collect::<Result<BTreeSet<_>, _>>()?;
    reclaim_disposable_bindings_for(db, candidates)?;
    Ok(())
}

/// Remove one graph record by local identity and return each binding whose
/// liveness may have changed. Relationship endpoints are the only other
/// identities that can become unused when the recursive edge subtree goes
/// away, so later cleanup can use their indexes directly.
fn remove_by_id(db: &Transaction<'_>, id: RefId) -> CwpdbResult<BTreeSet<RefId>> {
    let mut candidates = BTreeSet::new();
    candidates.insert(id);
    {
        let mut statement = db.prepare_cached(
            "WITH RECURSIVE doomed(id) AS (
                SELECT ?1
                UNION
                SELECT e.ref_id FROM edges e JOIN doomed d ON e.source_id=d.id OR e.target_id=d.id
            )
            SELECT e.ref_id, e.schema_id, e.source_id, e.target_id
            FROM edges e
            WHERE e.ref_id IN (SELECT id FROM doomed)",
        )?;
        for row in statement.query_map([id], |row| {
            Ok((
                row.get::<_, RefId>(0)?,
                row.get::<_, RefId>(1)?,
                row.get::<_, RefId>(2)?,
                row.get::<_, RefId>(3)?,
            ))
        })? {
            let (edge, schema, source, target) = row?;
            candidates.extend([edge, schema, source, target]);
        }
    }
    db.prepare_cached(
        "WITH RECURSIVE doomed(id) AS (
            SELECT ?1
            UNION
            SELECT e.ref_id FROM edges e JOIN doomed d ON e.source_id=d.id OR e.target_id=d.id
        )
        DELETE FROM edges WHERE ref_id IN (SELECT id FROM doomed)",
    )?
    .execute([id])?;
    db.prepare_cached("DELETE FROM nodes WHERE ref_id=?1")?
        .execute([id])?;
    Ok(candidates)
}

/// Reclaim only known candidates. Every predicate begins with an indexed
/// identity lookup, avoiding graph-wide correlated scans during retention.
fn reclaim_disposable_bindings_for(
    db: &Transaction<'_>,
    candidates: impl IntoIterator<Item = RefId>,
) -> CwpdbResult<()> {
    let mut statement = db.prepare_cached(
        r#"
            DELETE FROM ref_bindings
            WHERE ref_id=?1
              AND disposable=1
              AND NOT EXISTS (SELECT 1 FROM nodes WHERE ref_id=?1)
              AND NOT EXISTS (SELECT 1 FROM nodes WHERE schema_id=?1)
              AND NOT EXISTS (SELECT 1 FROM edges WHERE ref_id=?1)
              AND NOT EXISTS (SELECT 1 FROM edges WHERE schema_id=?1)
              AND NOT EXISTS (SELECT 1 FROM edges WHERE source_id=?1)
              AND NOT EXISTS (SELECT 1 FROM edges WHERE target_id=?1)
        "#,
    )?;
    for candidate in candidates {
        statement.execute([candidate])?;
    }
    Ok(())
}

/// Remove one directed relationship by its deterministic identity.
pub fn remove_edge(
    db: &Transaction<'_>,
    source: &str,
    kind: &str,
    target: &str,
) -> CwpdbResult<()> {
    remove(db, &derived_ref(kind, &[source, target]))
}

/// Replace all targets for one relationship kind, preserving listed order.
pub fn replace_targets(
    db: &Transaction<'_>,
    source: &str,
    kind: &str,
    new: &[String],
) -> CwpdbResult<()> {
    let selected = new.iter().collect::<std::collections::BTreeSet<_>>();
    for target in targets(db, source, kind)? {
        if !selected.contains(&target) {
            remove_edge(db, source, kind, &target)?;
        }
    }
    for (position, target) in new.iter().enumerate() {
        put_edge(
            db,
            source,
            kind,
            target,
            i64::try_from(position)?,
            &serde_json::json!({}),
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub mod declared;
pub mod desktop;

pub mod conversation;
pub mod schedules;

pub mod posthog;
mod sequence_accounting_ledger;
mod sequence_ledger;

mod usage;

mod personal_collections;
mod sidebar_pin_move;
mod sidebar_pins;
