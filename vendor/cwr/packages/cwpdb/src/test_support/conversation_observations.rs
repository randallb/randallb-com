//! Finite storage-owned observations for retained protocol compatibility tests.
use crate::CwpdbResult;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

/// Observe the native header clock used by the retained conversation projection.
/// Ownership, runtime identity and both physical clocks are read together; no
/// view installation, schema repair or caller-supplied query is exposed.
pub fn header_revision(path: &Path, runtime: &str, conversation: &str) -> CwpdbResult<i64> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(db.query_row("SELECT MAX(COALESCE(json_extract(n.props,'$.commit_revision'),0),COALESCE(json_extract(n.props,'$.timeline_revision'),0)) FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id JOIN edges own ON own.target_id=n.ref_id AND own.kind='conversation' JOIN ref_bindings root ON root.ref_id=own.source_id JOIN nodes runtime ON runtime.ref_id=root.ref_id AND runtime.kind='runtime' WHERE n.kind='conversation' AND r.canonical_ref=?1 AND root.canonical_ref=?2 AND json_extract(runtime.props,'$.runtime_root_ref')=?3",rusqlite::params![conversation,format!("{runtime}/cwpdb"),runtime],|r|r.get(0))?)
}
/// Read the retained message's exact encoded payload length for browse-limit assertions.
pub fn message_payload_bytes(path: &Path, message: &str) -> CwpdbResult<i64> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.execute_batch(include_str!(
        "../conversation/native/read_snapshot/read_views.sql"
    ))?;
    Ok(db.query_row(
        "SELECT length(payload_json) FROM messages WHERE message_ref=?1",
        [message],
        |row| row.get(0),
    )?)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn header_clock_preserves_both_revision_sources_and_refuses_foreign_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clock.sqlite");
        let runtime = "cwp://fixture/runtime";
        let conversation = "cwp://fixture/conversation";
        let root = format!("{runtime}/cwpdb");
        let mut store = crate::NativeStore::open(&path).unwrap();
        store
            .graph_transaction(|tx| {
                crate::put_node(tx, &root, "runtime", &json!({"runtime_root_ref":runtime}))?;
                crate::put_node(
                    tx,
                    conversation,
                    "conversation",
                    &json!({"commit_revision":6,"timeline_revision":9}),
                )?;
                crate::put_edge(tx, &root, "conversation", conversation, 0, &json!({}))?;
                Ok(())
            })
            .unwrap();
        assert_eq!(header_revision(&path, runtime, conversation).unwrap(), 9);
        assert!(header_revision(&path, "cwp://fixture/foreign", conversation).is_err());
        assert!(header_revision(&path, runtime, "cwp://fixture/foreign").is_err());
        store
            .graph_transaction(|tx| {
                crate::put_node(
                    tx,
                    conversation,
                    "conversation",
                    &json!({"commit_revision":11}),
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(header_revision(&path, runtime, conversation).unwrap(), 11);
    }
}
