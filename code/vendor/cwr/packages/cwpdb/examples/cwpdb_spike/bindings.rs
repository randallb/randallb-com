use rusqlite::{Connection, OptionalExtension};

use super::model::*;

/// Database-local keys never cross the CLI/API boundary.
pub type RefId = i64;

pub fn find(db: &Connection, reference: &str) -> Result<Option<RefId>> {
    Ok(db
        .prepare_cached("SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1")?
        .query_row([reference], |r| r.get(0))
        .optional()?)
}

pub fn resolve(db: &Connection, reference: &str) -> Result<RefId> {
    find(db, reference)?.ok_or_else(|| format!("unknown reference: {reference}").into())
}

/// Call inside the same transaction that inserts the node or edge.
pub fn bind(db: &Connection, reference: &str) -> Result<RefId> {
    ensure(
        !db.is_autocommit(),
        "reference bindings require a transaction",
    )?;
    if let Some(id) = find(db, reference)? {
        return Ok(id);
    }
    Ok(db
        .prepare_cached("INSERT INTO ref_bindings(canonical_ref) VALUES (?1) RETURNING ref_id")?
        .query_row([reference], |r| r.get(0))?)
}

pub fn expand(db: &Connection, id: RefId) -> Result<String> {
    Ok(db
        .prepare_cached("SELECT canonical_ref FROM ref_bindings WHERE ref_id=?1")?
        .query_row([id], |r| r.get(0))?)
}
