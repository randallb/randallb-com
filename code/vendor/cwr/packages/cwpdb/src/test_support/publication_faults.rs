//! Specific retained-publication faults used by downstream protocol tests.
use crate::{CwpdbResult, NativeStore};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

/// Toggle the one fixed message-update failure used to verify streaming rollback.
/// The database must already exist; the fixture cannot create another store.
pub fn reject_streaming_updates(path: &Path, reject: bool) -> CwpdbResult<()> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    if reject {
        db.execute_batch("CREATE TRIGGER reject_stream BEFORE UPDATE ON nodes WHEN NEW.kind='message' BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;")?;
    } else {
        db.execute_batch("DROP TRIGGER reject_stream;")?;
    }
    Ok(())
}
/// Detach one owned source message from its conversation without deleting the
/// independently retained publication placement or advancing the runtime clock.
/// This deliberately models the fixed unavailable-source compatibility case.
pub fn detach_source_message(
    path: &Path,
    runtime: &str,
    conversation: &str,
    message: &str,
) -> CwpdbResult<()> {
    if !path.is_file() {
        return Err("publication fixture requires an existing database".into());
    }
    let mut store = NativeStore::open(path)?;
    store.graph_transaction(|tx| {
        let root=format!("{runtime}/cwpdb");
        let metadata=crate::node_props(tx,&root)?.ok_or("fixture runtime missing")?;
        if metadata["runtime_root_ref"] != runtime { return Err("fixture runtime identity mismatch".into()); }
        let owned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes conversation ON conversation.ref_id=e.target_id AND conversation.kind='conversation' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='conversation' AND e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2))",rusqlite::params![root,conversation],|r|r.get(0))?;
        if !owned { return Err("fixture conversation is not owned by runtime".into()); }
        let linked:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes message ON message.ref_id=e.target_id AND message.kind='message' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='message' AND e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2))",rusqlite::params![conversation,message],|r|r.get(0))?;
        if !linked { return Err("fixture source message is not owned by conversation".into()); }
        let removed=tx.execute("DELETE FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='message' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)",rusqlite::params![conversation,message])?;
        if removed!=1 {return Err("fixture source membership changed".into());}
        Ok(())
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn fixed_streaming_failure_rolls_back_then_allows_the_same_update() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stream.sqlite");
        let message = "cwp://fixture/message";
        let mut store = NativeStore::open(&path).unwrap();
        store
            .graph_transaction(|tx| {
                crate::put_node(tx, message, "message", &json!({"text":"old"}))?;
                Ok(())
            })
            .unwrap();
        reject_streaming_updates(&path, true).unwrap();
        assert!(store
            .graph_transaction(|tx| {
                crate::put_node(tx, message, "message", &json!({"text":"new"}))?;
                Ok(())
            })
            .is_err());
        assert_eq!(
            store
                .graph_read(|db| crate::node_props(db, message))
                .unwrap()
                .unwrap()["text"],
            "old"
        );
        reject_streaming_updates(&path, false).unwrap();
        store
            .graph_transaction(|tx| {
                crate::put_node(tx, message, "message", &json!({"text":"new"}))?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store
                .graph_read(|db| crate::node_props(db, message))
                .unwrap()
                .unwrap()["text"],
            "new"
        );
    }
    #[test]
    fn detachment_checks_ownership_and_preserves_message_placement_and_clock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.sqlite");
        let runtime = "cwp://fixture/runtime";
        let root = format!("{runtime}/cwpdb");
        let conversation = "cwp://fixture/conversation";
        let message = "cwp://fixture/message";
        let mut store = NativeStore::open(&path).unwrap();
        store
            .graph_transaction(|tx| {
                for (reference, kind, value) in [
                    (
                        &root[..],
                        "runtime",
                        json!({"runtime_root_ref":runtime,"commit_revision":4}),
                    ),
                    (conversation, "conversation", json!({})),
                    (message, "message", json!({"text":"retained"})),
                ] {
                    crate::put_node(tx, reference, kind, &value)?;
                }
                crate::put_edge(tx, &root, "conversation", conversation, 0, &json!({}))?;
                crate::put_edge(tx, conversation, "message", message, 0, &json!({}))?;
                crate::put_edge(tx, conversation, "timeline_message", message, 0, &json!({}))?;
                Ok(())
            })
            .unwrap();
        assert!(detach_source_message(&path, runtime, "cwp://fixture/foreign", message).is_err());
        assert!(
            detach_source_message(&path, "cwp://fixture/foreign", conversation, message).is_err()
        );
        store
            .graph_read(|db| {
                assert_eq!(crate::targets(db, conversation, "message")?, vec![message]);
                Ok(())
            })
            .unwrap();
        detach_source_message(&path, runtime, conversation, message).unwrap();
        store
            .graph_read(|db| {
                assert!(crate::targets(db, conversation, "message")?.is_empty());
                assert_eq!(
                    crate::targets(db, conversation, "timeline_message")?,
                    vec![message]
                );
                assert_eq!(crate::node_props(db, message)?.unwrap()["text"], "retained");
                assert_eq!(crate::node_props(db, &root)?.unwrap()["commit_revision"], 4);
                Ok(())
            })
            .unwrap();
        assert!(detach_source_message(&path, runtime, conversation, message).is_err());
    }
}
