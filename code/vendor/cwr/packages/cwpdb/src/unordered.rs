//! Trusted domain-owned unordered relationships. No wire caller selects these
//! contracts. Unlink advances the durable source revision, so re-establishing
//! a tuple receives a fresh comparison token without a second revision clock.
use crate::{
    ref_read::{get_ref, RefReadError},
    CwpdbResult,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
pub struct Contract {
    pub kind: &'static str,
    pub schema: &'static str,
    pub source_kind: &'static str,
    pub target_kind: &'static str,
    pub external_target: bool,
    pub source_cardinality: Cardinality,
    pub target_cardinality: Cardinality,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    One,
    Many,
}

#[derive(Debug)]
pub enum Error {
    WorkLimit,
    Endpoint,
    Conflict,
    Cardinality,
    RevisionConflict,
    RelationshipsExist,
    Storage(Box<dyn std::error::Error + Send + Sync>),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unordered relationship: {self:?}")
    }
}
impl std::error::Error for Error {}
fn storage(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Error {
    Error::Storage(e.into())
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub target: String,
    pub revision: i64,
}

/// Existing Desktop identities hash JSON, rather than introducing a new key.
pub fn desktop_ref(kind: &str, key: &Value) -> String {
    let hash = Sha256::digest(serde_json::to_vec(key).expect("JSON value serializes"));
    format!("cwp://app.getworkloop.local/{kind}/{}", hex::encode(hash))
}

pub fn single_outgoing(
    db: &Connection,
    contract: Contract,
    source: &str,
) -> Result<Option<Edge>, Error> {
    let read = || -> CwpdbResult<_> {
        let mut query = db.prepare_cached("SELECT e.kind,s.canonical_ref,t.canonical_ref,e.revision,json(e.props),e.position,r.canonical_ref FROM edges e JOIN ref_bindings t ON t.ref_id=e.target_id JOIN ref_bindings s ON s.ref_id=e.schema_id JOIN ref_bindings r ON r.ref_id=e.ref_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind=?2 LIMIT 2")?;
        let rows = query
            .query_map(params![source, contract.kind], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    };
    let rows = read().map_err(storage)?;
    if rows.len() > 1 {
        return Err(Error::Cardinality);
    }
    let Some((_, schema, target, revision, props, position, reference)) = rows.into_iter().next()
    else {
        return Ok(None);
    };
    if schema != contract.schema
        || position != 0
        || serde_json::from_str::<Value>(&props).map_err(storage)? != json!({"record":{}})
        || reference != desktop_ref(contract.kind, &json!([source, target]))
    {
        return Err(Error::Conflict);
    }
    Ok(Some(Edge { target, revision }))
}

fn exact(
    db: &Connection,
    contract: Contract,
    source: &str,
    target: &str,
) -> Result<Option<Edge>, Error> {
    let row = db.query_row("SELECT s.canonical_ref,e.revision,json(e.props),e.position,r.canonical_ref FROM edges e JOIN ref_bindings s ON s.ref_id=e.schema_id JOIN ref_bindings r ON r.ref_id=e.ref_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind=?2 AND e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3)", params![source,contract.kind,target], |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?))).optional().map_err(storage)?;
    let Some((schema, revision, props, position, reference)) = row else {
        return Ok(None);
    };
    if schema != contract.schema
        || position != 0
        || serde_json::from_str::<Value>(&props).map_err(storage)? != json!({"record":{}})
        || reference != desktop_ref(contract.kind, &json!([source, target]))
    {
        return Err(Error::Conflict);
    }
    Ok(Some(Edge {
        target: target.into(),
        revision,
    }))
}

pub fn link(
    tx: &Transaction<'_>,
    contract: Contract,
    source: &str,
    target: &str,
) -> Result<(), Error> {
    let source_record = get_ref(tx, source).map_err(storage)?;
    if source_record.kind != contract.source_kind || source == target {
        return Err(Error::Endpoint);
    }
    match get_ref(tx, target) {
        Ok(record) if contract.target_kind != record.kind => return Err(Error::Endpoint),
        Ok(_) => {}
        Err(RefReadError::NotFound)
            if contract.external_target
                && crate::identity_claim(tx, target)
                    .map_err(storage)?
                    .is_none() => {}
        Err(RefReadError::NotFound | RefReadError::WrongRecordType) => return Err(Error::Endpoint),
        Err(e) => return Err(storage(e)),
    }
    if exact(tx, contract, source, target)?.is_some() {
        return Ok(());
    }
    if contract.source_cardinality == Cardinality::One
        && single_outgoing(tx, contract, source)?.is_some()
    {
        return Err(Error::Cardinality);
    }
    let target_id = crate::bind(tx, target).map_err(storage)?;
    let source_id = crate::bind(tx, source).map_err(storage)?;
    if contract.target_cardinality == Cardinality::One {
        let competing: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE target_id=?1 AND kind=?2 AND source_id!=?3)", params![target_id,contract.kind,source_id], |r|r.get(0)).map_err(storage)?;
        if competing {
            return Err(Error::Cardinality);
        }
    }
    let reference = desktop_ref(contract.kind, &json!([source, target]));
    let id = crate::identity::claim_record_identity(
        tx,
        &reference,
        &format!("edge:{}", contract.kind),
        crate::BindingRetention::Durable,
    )
    .map_err(storage)?;
    let schema = crate::bind(tx, contract.schema).map_err(storage)?;
    tx.execute("INSERT INTO edges(ref_id,kind,schema_id,source_id,target_id,position,revision,props) VALUES(?1,?2,?3,?4,?5,0,?6,jsonb(?7))",params![id,contract.kind,schema,source_id,target_id,source_record.revision.legacy_counter(),r#"{"record":{}}"#]).map_err(storage)?;
    Ok(())
}

pub fn unlink(
    tx: &Transaction<'_>,
    contract: Contract,
    source: &str,
    target: &str,
    expected: i64,
) -> Result<(), Error> {
    let Some(edge) = exact(tx, contract, source, target)? else {
        return Ok(());
    };
    if edge.target != target || edge.revision != expected {
        return Err(Error::RevisionConflict);
    }
    let reference = desktop_ref(contract.kind, &json!([source, target]));
    let id = crate::find(tx, &reference)
        .map_err(storage)?
        .ok_or(Error::Conflict)?;
    let incident: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=?1 OR target_id=?1)",
            [id],
            |r| r.get(0),
        )
        .map_err(storage)?;
    if incident {
        return Err(Error::RelationshipsExist);
    }
    // Adopt only this already-validated legacy edge's exact semantic role.
    crate::identity::claim_record_identity(
        tx,
        &reference,
        &format!("edge:{}", contract.kind),
        crate::BindingRetention::Durable,
    )
    .map_err(storage)?;
    // Removing any membership advances its durable owner's epoch before the
    // tuple can be re-established. This avoids both per-edge tombstone records
    // and reuse of an old edge token. The whole domain transaction may roll back.
    let advanced=tx.execute("UPDATE nodes SET revision=revision+1 WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind=?2 AND revision<9223372036854775807",params![source,contract.source_kind]).map_err(storage)?;
    if advanced != 1 {
        return Err(Error::RevisionConflict);
    }
    tx.execute("DELETE FROM edges WHERE ref_id=?1", [id])
        .map_err(storage)?;
    Ok(())
}

/// Reserve a validated retained value's identity without rewriting its payload.
pub fn adopt_entity(tx: &Transaction<'_>, reference: &str, kind: &str) -> CwpdbResult<()> {
    let record = get_ref(tx, reference)?;
    if record.kind != kind {
        return Err("entity definition mismatch".into());
    }
    crate::identity::claim_record_identity(
        tx,
        reference,
        &format!("node:{kind}"),
        crate::BindingRetention::Durable,
    )?;
    Ok(())
}

/// No cascading fallback: domains must explicitly unlink their allowed kinds.
pub fn delete_unlinked_entity(tx: &Transaction<'_>, reference: &str) -> Result<(), Error> {
    let Some(id) = crate::find(tx, reference).map_err(storage)? else {
        return Ok(());
    };
    require_unlinked(tx, reference)?;
    tx.execute("DELETE FROM nodes WHERE ref_id=?1", [id])
        .map_err(storage)?;
    Ok(())
}

/// Constant indexed lookup used to distinguish retired values before creation.
pub fn retired(db: &Connection, reference: &str) -> CwpdbResult<bool> {
    let row: Option<Option<String>> = db
        .query_row(
            "SELECT claim_kind FROM ref_bindings WHERE canonical_ref=?1",
            [reference],
            |r| r.get(0),
        )
        .optional()?;
    Ok(row.flatten().is_some())
}

#[cfg(test)]
#[path = "unordered_tests.rs"]
mod tests;

/// Explicit complete-or-limit read for a bounded domain transaction. SQL uses
/// the existing source/kind index; no prefix can be mistaken for a complete set.
pub fn many_outgoing(
    db: &Connection,
    contract: Contract,
    source: &str,
    limit: usize,
) -> Result<Vec<Edge>, Error> {
    if limit == 0 || limit > 20_000 {
        return Err(Error::WorkLimit);
    }
    let mut statement=db.prepare_cached("SELECT t.canonical_ref,e.revision,s.canonical_ref,e.position,json(e.props),r.canonical_ref FROM edges e JOIN ref_bindings t ON t.ref_id=e.target_id JOIN ref_bindings s ON s.ref_id=e.schema_id JOIN ref_bindings r ON r.ref_id=e.ref_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind=?2 LIMIT ?3").map_err(storage)?;
    let rows = statement
        .query_map(params![source, contract.kind, (limit + 1) as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })
        .map_err(storage)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(storage)?;
    if rows.len() > limit {
        return Err(Error::WorkLimit);
    }
    rows.into_iter()
        .map(|(target, revision, schema, position, props, reference)| {
            if schema != contract.schema
                || position != 0
                || serde_json::from_str::<Value>(&props).map_err(storage)? != json!({"record":{}})
                || reference != desktop_ref(contract.kind, &json!([source, target]))
            {
                return Err(Error::Conflict);
            }
            Ok(Edge { target, revision })
        })
        .collect()
}

pub fn require_unlinked(tx: &Transaction<'_>, reference: &str) -> Result<(), Error> {
    let Some(id) = crate::find(tx, reference).map_err(storage)? else {
        return Ok(());
    };
    let incident: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=?1 OR target_id=?1)",
            [id],
            |r| r.get(0),
        )
        .map_err(storage)?;
    if incident {
        Err(Error::RelationshipsExist)
    } else {
        Ok(())
    }
}
