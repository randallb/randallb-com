//! Fixed legacy invocation/read fixtures; no physical handle escapes CWPDB.
use crate::CwpdbResult;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
fn existing(path: &Path) -> CwpdbResult<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?)
}
/// Remove exactly the retained attempt boundary to model older resumed records.
pub fn forget_attempt_boundary(path: &Path, invocation: &str) -> CwpdbResult<()> {
    let db = existing(path)?;
    db.execute("UPDATE nodes SET props=jsonb_remove(props,'$.record.attempt_started_at_unix_ms','$.record.attempt_started_after_activity_sequence') WHERE kind='invocation' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[invocation])?;
    Ok(())
}
/// Remove explicit reply identities from legacy invocations.
pub fn forget_reply_identities(path: &Path) -> CwpdbResult<()> {
    existing(path)?.execute("UPDATE nodes SET props=jsonb_remove(props,'$.record.reply_message_ref') WHERE kind='invocation'",[])?;
    Ok(())
}
/// Model an explicitly misowned retained reply; protocol readers must reject it.
pub fn cross_conversation_reply(
    path: &Path,
    invocation: &str,
    message: &str,
    conversation: &str,
) -> CwpdbResult<()> {
    existing(path)?.execute("UPDATE nodes SET props=jsonb_set(props,'$.record.reply_message_ref',?1,'$.record.conversation_ref',?2) WHERE kind='invocation' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3)",rusqlite::params![message,conversation,invocation])?;
    Ok(())
}
/// Toggle the fixed invocation update fault used to exercise atomic rollback.
pub fn reject_invocation_updates(path: &Path, reject: bool) -> CwpdbResult<()> {
    let db = existing(path)?;
    if reject {
        db.execute_batch("CREATE TRIGGER reject_invocation_update BEFORE UPDATE ON nodes WHEN OLD.kind='invocation' BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;")?;
    } else {
        db.execute_batch("DROP TRIGGER reject_invocation_update")?;
    }
    Ok(())
}
/// Exact retained message byte length, independently of browse's bounded port.
pub fn message_payload_bytes(path: &Path, message: &str) -> CwpdbResult<i64> {
    Ok(existing(path)?.query_row("SELECT length(CAST(CASE WHEN json_type(e.props,'$.visualizations') IS NULL THEN json_extract(n.props,'$.record') ELSE json_set(json_extract(n.props,'$.record'),'$.visualizations',json_extract(e.props,'$.visualizations')) END AS BLOB)) FROM edges e JOIN nodes n ON n.ref_id=e.target_id AND n.kind='message' WHERE e.kind='message' AND n.ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[message],|r|r.get(0))?)
}
/// Current native runtime clock, without exposing a connection.
pub fn commit_revision(path: &Path, runtime: &str) -> CwpdbResult<i64> {
    Ok(existing(path)?.query_row("SELECT json_extract(props,'$.commit_revision') FROM nodes WHERE kind='runtime' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[format!("{runtime}/cwpdb")],|r|r.get(0))?)
}
/// Introduce one legacy private wait with a response that must never be published.
pub fn private_wait(path: &Path, waiter: &str, awaited: &str, reference: &str) -> CwpdbResult<()> {
    let mut db = existing(path)?;
    let tx = db.transaction()?;
    tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.record.state','waiting','$.record.attempt_active',json('false')) WHERE kind='invocation' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[waiter])?;
    crate::put_node(
        &tx,
        reference,
        "wait",
        &serde_json::json!({"record":{"waiter_invocation_ref":waiter,"awaited_invocation_ref":awaited,"call_id":"waited-call","state":"pending","response":{"private":"never expose"}}}),
    )?;
    crate::put_edge(
        &tx,
        reference,
        "field:waiter_invocation_ref",
        waiter,
        0,
        &serde_json::json!({}),
    )?;
    tx.commit()?;
    Ok(())
}
/// Legacy detached child whose parent has become terminal.
pub fn finish_parent(path: &Path, parent: &str) -> CwpdbResult<()> {
    existing(path)?.execute("UPDATE nodes SET props=jsonb_set(props,'$.record.state','completed','$.record.finished_at_unix_ms',50) WHERE kind='invocation' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[parent])?;
    Ok(())
}
/// Header-only empty conversation for invocation scope refusal assertions.
pub fn empty_conversation(path: &Path, runtime: &str, conversation: &str) -> CwpdbResult<()> {
    let mut db = existing(path)?;
    let tx = db.transaction()?;
    crate::put_node(
        &tx,
        conversation,
        "conversation",
        &serde_json::json!({"creation_order":2,"commit_revision":1}),
    )?;
    crate::put_edge(
        &tx,
        &format!("{runtime}/cwpdb"),
        "conversation",
        conversation,
        2,
        &serde_json::json!({}),
    )?;
    tx.commit()?;
    Ok(())
}
/// Number of retained flat activity projections for one conversation.
pub fn retained_activity_count(path: &Path, conversation: &str) -> CwpdbResult<i64> {
    Ok(existing(path)?.query_row("SELECT COUNT(*) FROM edges e JOIN ref_bindings r ON r.ref_id=e.source_id WHERE e.kind='timeline_activity' AND r.canonical_ref=?1",[conversation],|r|r.get(0))?)
}
/// Reproduce the obsolete work-update attachment in retained flat projections.
pub fn legacy_work_attachments(path: &Path) -> CwpdbResult<usize> {
    Ok(existing(path)?.execute("UPDATE edges SET props=jsonb_set(props,'$.record.event.attachment',jsonb('{}')) WHERE kind='timeline_activity' AND json_extract(props,'$.record.event.kind')='work_update'",[])?)
}
/// Observe whether rewriting removed all obsolete flat activity attachments.
pub fn legacy_attachment_count(path: &Path) -> CwpdbResult<i64> {
    Ok(existing(path)?.query_row("SELECT COUNT(*) FROM edges WHERE kind='timeline_activity' AND json_extract(props,'$.record.event.attachment') IS NOT NULL",[],|r|r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_observation_counts_utf8_bytes_and_fault_preserves_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("read-fixture.sqlite");
        assert!(reject_invocation_updates(&path, true).is_err());
        assert!(!path.exists());
        let mut store = crate::NativeStore::open(&path).unwrap();
        let message = "cwp://fixture/message";
        let invocation = "cwp://fixture/invocation";
        let record = serde_json::json!({"message_ref":message,"text":"🌵é"});
        store
            .graph_transaction(|tx| {
                crate::put_node(
                    tx,
                    message,
                    "message",
                    &serde_json::json!({"record":record}),
                )?;
                crate::put_node(
                    tx,
                    "cwp://fixture/conversation",
                    "conversation",
                    &serde_json::json!({}),
                )?;
                crate::put_edge(
                    tx,
                    "cwp://fixture/conversation",
                    "message",
                    message,
                    0,
                    &serde_json::json!({}),
                )?;
                crate::put_node(
                    tx,
                    invocation,
                    "invocation",
                    &serde_json::json!({"record":{"state":"running"}}),
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            message_payload_bytes(&path, message).unwrap() as usize,
            serde_json::to_vec(&record).unwrap().len()
        );
        reject_invocation_updates(&path, true).unwrap();
        let update = |store: &mut crate::NativeStore| {
            store.graph_transaction(|tx| {
                crate::put_node(
                    tx,
                    invocation,
                    "invocation",
                    &serde_json::json!({"record":{"state":"completed"}}),
                )
            })
        };
        assert!(update(&mut store).is_err());
        store
            .graph_read(|db| {
                assert_eq!(
                    crate::node_props(db, invocation)?.unwrap()["record"]["state"],
                    "running"
                );
                Ok(())
            })
            .unwrap();
        reject_invocation_updates(&path, false).unwrap();
        assert!(update(&mut store).is_ok());
    }
}
