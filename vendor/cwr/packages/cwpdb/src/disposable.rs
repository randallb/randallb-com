//! Storage-owned disposable entity lifecycle. No retained request replay.
use crate::{
    identity::{claim_record_identity, identity_claim},
    ref_read::{get_ref, RefReadError, RefRecord, RefRevision},
    CwpdbResult,
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;

pub use crate::BindingRetention;

// Storage metadata uses the existing graph, with one durable clock record
// instead of per-observation tombstones or an additional physical table.
const CLOCK_REF: &str = "cwp://org.coworkerprotocol/cwpdb/disposable-revision-clock";
const CLOCK_KIND: &str = "disposable_revision_clock";
const CLOCK_SCHEMA: &str =
    "cwp://org.coworkerprotocol/cwpdb/schemas/disposable-revision-clock/2026-10-23";
fn next_revision(tx: &Transaction<'_>) -> CwpdbResult<i64> {
    let previous = match get_ref(tx, CLOCK_REF) {
        Ok(record) => {
            if record.kind != CLOCK_KIND || record.value.schema_ref != CLOCK_SCHEMA {
                return Err("unsupported disposable revision clock".into());
            }
            record
                .value
                .properties
                .get("value")
                .and_then(Value::as_i64)
                .filter(|value| *value >= 0)
                .ok_or("invalid disposable revision clock")?
        }
        Err(RefReadError::NotFound) => 0,
        Err(error) => return Err(error.into()),
    };
    let next = previous
        .checked_add(1)
        .ok_or("disposable revision clock exhausted")?;
    crate::put_node_with_schema(
        tx,
        CLOCK_REF,
        CLOCK_KIND,
        CLOCK_SCHEMA,
        &serde_json::json!({"value":next}),
    )?;
    Ok(next)
}

pub fn insert_disposable_entity(
    tx: &Transaction<'_>,
    reference: &str,
    kind: &str,
    schema: &str,
    value: &Value,
) -> CwpdbResult<RefRecord> {
    if !value.is_object() {
        return Err("entity value must be an object".into());
    }
    if serde_json::to_vec(value)?.len() > 1_048_576 {
        return Err("entity byte limit exceeded".into());
    }
    match get_ref(tx, reference) {
        Ok(_) => return Err("already_exists".into()),
        Err(RefReadError::NotFound) => {}
        Err(e) => return Err(e.into()),
    }
    // An unused durable binding is reserved, even without a live value.
    let binding: Option<bool> = tx
        .query_row(
            "SELECT disposable FROM ref_bindings WHERE canonical_ref=?1",
            [reference],
            |r| r.get(0),
        )
        .optional()?;
    if binding == Some(false) || identity_claim(tx, reference)?.is_some() {
        return Err("ref_retired_or_claimed".into());
    }
    if reference == schema {
        return Err("entity ref is reserved for a schema".into());
    }
    let revision = next_revision(tx)?;
    let id = claim_record_identity(
        tx,
        reference,
        &format!("node:{kind}"),
        BindingRetention::Disposable,
    )?;
    let schema_id = crate::bind(tx, schema)?;
    tx.execute(
        "INSERT INTO nodes(ref_id,kind,schema_id,revision,props) VALUES(?1,?2,?3,?4,jsonb(?5))",
        params![id, kind, schema_id, revision, serde_json::to_string(value)?],
    )?;
    Ok(get_ref(tx, reference)?)
}

pub fn replace_disposable_entity(
    tx: &Transaction<'_>,
    reference: &str,
    kind: &str,
    schema: &str,
    expected: &RefRevision,
    value: &Value,
) -> CwpdbResult<RefRecord> {
    let current = get_ref(tx, reference)?;
    if current.kind != kind || current.value.schema_ref != schema {
        return Err("entity definition mismatch".into());
    }
    if &current.revision != expected {
        return Err("entity revision conflict".into());
    }
    if !value.is_object() {
        return Err("entity value must be an object".into());
    }
    if serde_json::to_vec(value)?.len() > 1_048_576 {
        return Err("entity byte limit exceeded".into());
    }
    let disposable: bool = tx.query_row(
        "SELECT disposable FROM ref_bindings WHERE canonical_ref=?1",
        [reference],
        |r| r.get(0),
    )?;
    if !disposable {
        return Err("entity has durable identity".into());
    }
    let revision = next_revision(tx)?;
    tx.execute(
        "UPDATE nodes SET props=jsonb(?1),revision=?2 WHERE ref_id=(
        SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3)",
        params![serde_json::to_string(value)?, revision, reference],
    )?;
    Ok(get_ref(tx, reference)?)
}

/// Validate the entire selection before removing any records. Limits include
/// recursively incident relationships, including relationships to edges.
pub fn purge_disposable_entities(
    tx: &Transaction<'_>,
    records: &[(&str, &RefRevision)],
) -> CwpdbResult<()> {
    if records.len() > 250 {
        return Err("disposable batch limit exceeded".into());
    }
    let mut work = 0i64;
    for (reference, expected) in records {
        let current = get_ref(tx, reference)?;
        if &current.revision != *expected {
            return Err("entity revision conflict".into());
        }
        let (count, durable): (i64,bool) = tx.query_row(
            "WITH RECURSIVE doomed(id) AS (
                SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1 UNION
                SELECT e.ref_id FROM edges e JOIN doomed d ON e.source_id=d.id OR e.target_id=d.id LIMIT 1025
            ) SELECT count(*),coalesce(max(b.disposable=0),0) FROM doomed d JOIN ref_bindings b ON b.ref_id=d.id",
            [reference], |r| Ok((r.get(0)?,r.get(1)?)))?;
        work += count;
        if work > 1024 {
            return Err("disposable purge work limit exceeded".into());
        }
        if durable {
            return Err("disposable entity has durable references".into());
        }
    }
    crate::remove_disposables(tx, records.iter().map(|(reference, _)| *reference))
}
