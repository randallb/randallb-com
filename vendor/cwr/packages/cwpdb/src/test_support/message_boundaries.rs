//! Exact retained message-boundary corruption and observations for protocol tests.
use crate::CwpdbResult;
use rusqlite::{params, Connection, OpenFlags};
use std::path::Path;

fn open(path: &Path, runtime: &str, writable: bool) -> CwpdbResult<Connection> {
    let db = Connection::open_with_flags(
        path,
        if writable {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        },
    )?;
    let metadata =
        crate::node_props(&db, &format!("{runtime}/cwpdb"))?.ok_or("fixture runtime missing")?;
    if metadata["runtime_root_ref"] != runtime {
        return Err("fixture runtime identity mismatch".into());
    }
    Ok(db)
}

pub fn retained_records(path: &Path, runtime: &str) -> CwpdbResult<Vec<(String, String)>> {
    let db = open(path, runtime, false)?;
    let mut statement = db.prepare("SELECT r.canonical_ref,json_extract(n.props,'$.record') FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE n.kind IN ('activity','message','conversation_transfer_metadata') ORDER BY r.canonical_ref")?;
    let result = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(result)
}

pub fn remove_derived_lookup(path: &Path, runtime: &str) -> CwpdbResult<()> {
    let db = open(path, runtime, true)?;
    db.execute("DELETE FROM edges WHERE kind='field:event.message_ref'", [])?;
    Ok(())
}

pub fn stale_lookup(path: &Path, runtime: &str, activity: &str, message: &str) -> CwpdbResult<()> {
    let mut db = open(path, runtime, true)?;
    let tx = db.transaction()?;
    let record = crate::node_props(&tx, activity)?.ok_or("fixture activity missing")?;
    if record["record"]["event"]["kind"] != "message_recorded"
        || record["record"]["event"]["message_ref"] == message
    {
        return Err("fixture requires a different retained message boundary".into());
    }
    let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM nodes WHERE kind='message' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1))", [message], |row| row.get(0))?;
    if !exists {
        return Err("fixture message missing".into());
    }
    crate::put_edge(
        &tx,
        activity,
        "field:event.message_ref",
        message,
        0,
        &serde_json::json!({}),
    )?;
    tx.commit()?;
    Ok(())
}

pub fn lookup_count(path: &Path, runtime: &str, activity: Option<&str>) -> CwpdbResult<i64> {
    let db = open(path, runtime, false)?;
    Ok(match activity {
        Some(reference) => db.query_row("SELECT COUNT(*) FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='field:event.message_ref'", params![reference], |row|row.get(0))?,
        None => db.query_row("SELECT COUNT(*) FROM edges WHERE kind='field:event.message_ref'", [], |row|row.get(0))?,
    })
}
