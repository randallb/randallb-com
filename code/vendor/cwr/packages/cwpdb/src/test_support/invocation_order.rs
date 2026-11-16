//! A fixed timestamp tie used to exercise retained invocation admission order.
use crate::CwpdbResult;
use rusqlite::{params, Connection, OpenFlags};
use std::path::Path;

pub fn tie_admission_timestamps(
    path: &Path,
    runtime: &str,
    references: [&str; 2],
) -> CwpdbResult<()> {
    let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let tx = db.transaction()?;
    let root = format!("{runtime}/cwpdb");
    let metadata = crate::node_props(&tx, &root)?.ok_or("fixture runtime missing")?;
    if metadata["runtime_root_ref"] != runtime {
        return Err("fixture runtime identity mismatch".into());
    }
    for reference in references {
        let record = crate::node_props(&tx, reference)?.ok_or("fixture invocation missing")?;
        if record["record"]["invocation_ref"] != reference {
            return Err("fixture invocation identity mismatch".into());
        }
        let conversation = record["record"]["conversation_ref"]
            .as_str()
            .ok_or("fixture invocation conversation missing")?;
        let owned: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes n ON n.ref_id=e.target_id AND n.kind='conversation' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='conversation' AND e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2))", params![root, conversation], |row|row.get(0))?;
        if !owned {
            return Err("fixture invocation conversation is foreign".into());
        }
        let changed = tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.record.admitted_at_unix_ms',7) WHERE kind='invocation' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", [reference])?;
        if changed != 1 {
            return Err("fixture requires an existing invocation".into());
        }
    }
    tx.commit()?;
    Ok(())
}
