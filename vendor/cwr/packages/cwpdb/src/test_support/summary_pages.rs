//! Named summary relation fixtures and a fixed read-lock fixture.
use crate::CwpdbResult;
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::path::Path;

fn open(path: &Path, runtime: &str) -> CwpdbResult<Connection> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let metadata =
        crate::node_props(&db, &format!("{runtime}/cwpdb"))?.ok_or("fixture runtime missing")?;
    if metadata["runtime_root_ref"] != runtime {
        return Err("fixture runtime identity mismatch".into());
    }
    Ok(db)
}
fn insert(db: &rusqlite::Transaction<'_>, runtime: &str, index: usize) -> CwpdbResult<()> {
    let reference = format!("{runtime}/conversations/{index:05}");
    let root = format!("{runtime}/cwpdb");
    crate::put_node(
        db,
        &reference,
        "conversation",
        &json!({"creation_order":index,"title":format!("Conversation {index}"),"commit_revision":index+1,"last_message_ref":format!("{runtime}/messages/{index:05}"),"last_message_preview":format!("Preview {index}"),"completed_work_refs":[]}),
    )?;
    for kind in ["conversation", "root_conversation"] {
        crate::put_edge(
            db,
            &root,
            kind,
            &reference,
            i64::try_from(index)?,
            &json!({}),
        )?;
    }
    Ok(())
}
pub fn seed(path: &Path, runtime: &str, count: usize) -> CwpdbResult<()> {
    if count > 50000 {
        return Err("summary fixture count exceeds limit".into());
    }
    let mut db = open(path, runtime)?;
    let tx = db.transaction()?;
    for index in 0..count {
        insert(&tx, runtime, index)?;
    }
    tx.commit()?;
    Ok(())
}
pub fn append(path: &Path, runtime: &str, index: usize) -> CwpdbResult<()> {
    let mut db = open(path, runtime)?;
    let tx = db.transaction()?;
    insert(&tx, runtime, index)?;
    tx.commit()?;
    Ok(())
}
pub fn archive_first(path: &Path, runtime: &str) -> CwpdbResult<()> {
    let mut db = open(path, runtime)?;
    let tx = db.transaction()?;
    crate::put_edge(
        &tx,
        &format!("{runtime}/conversations/00000"),
        "archived",
        &format!("{runtime}/cwpdb"),
        0,
        &json!({}),
    )?;
    tx.commit()?;
    Ok(())
}
pub fn update_content_without_membership(path: &Path, runtime: &str) -> CwpdbResult<()> {
    let db = open(path, runtime)?;
    db.execute("UPDATE nodes SET props=jsonb_set(props,'$.title','Updated','$.last_message_preview','New message','$.commit_revision',99) WHERE kind='conversation'", [])?;
    Ok(())
}
pub fn rollback_journal(path: &Path, runtime: &str) -> CwpdbResult<()> {
    open(path, runtime)?.pragma_update(None, "journal_mode", "DELETE")?;
    Ok(())
}
/// Only acquisition, release and drop are exposed; no native query or handle escapes.
pub struct ExclusiveReadLock {
    db: Connection,
    active: bool,
}
impl ExclusiveReadLock {
    pub fn acquire(path: &Path, runtime: &str) -> CwpdbResult<Self> {
        let db = open(path, runtime)?;
        db.execute_batch("BEGIN EXCLUSIVE")?;
        Ok(Self { db, active: true })
    }
    pub fn release(&mut self) -> CwpdbResult<()> {
        if self.active {
            self.db.execute_batch("ROLLBACK")?;
            self.active = false;
        }
        Ok(())
    }
}
impl Drop for ExclusiveReadLock {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

pub fn owner_context(
    path: &Path,
    runtime: &str,
    conversation: &str,
    owner: &str,
) -> CwpdbResult<()> {
    let db = open(path, runtime)?;
    let changed = db.execute("UPDATE nodes SET props=jsonb_set(props,'$.owner_context_ref',jsonb(?2)) WHERE kind='conversation' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", rusqlite::params![conversation, serde_json::to_string(owner)?])?;
    if changed != 1 {
        return Err("fixture conversation missing".into());
    }
    Ok(())
}
pub fn participant(
    path: &Path,
    runtime: &str,
    conversation: &str,
    actor: &str,
    present: bool,
) -> CwpdbResult<()> {
    let mut db = open(path, runtime)?;
    let tx = db.transaction()?;
    if present {
        crate::put_edge(&tx, conversation, "participant", actor, 0, &json!({}))?;
    } else {
        tx.execute("DELETE FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND kind='participant'", rusqlite::params![conversation, actor])?;
    }
    tx.commit()?;
    Ok(())
}
pub fn completed_root_reference(
    path: &Path,
    runtime: &str,
    conversation: &str,
    latest: &str,
) -> CwpdbResult<()> {
    let db = open(path, runtime)?;
    let changed = db.execute("UPDATE nodes SET props=jsonb_set(props,'$.completed_work_refs',json_array(?2)) WHERE kind='conversation' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", rusqlite::params![conversation, latest])?;
    if changed != 1 {
        return Err("fixture conversation missing".into());
    }
    Ok(())
}
pub fn ambiguous_legacy_sidebar(path: &Path, runtime: &str) -> CwpdbResult<()> {
    let mut db = open(path, runtime)?;
    let tx = db.transaction()?;
    for index in [1, 3] {
        crate::put_node(
            &tx,
            &format!("{runtime}/conversations/{index:05}"),
            "conversation",
            &json!({"sidebar_eligible":false}),
        )?;
    }
    let legacy = format!("{runtime}/conversations/00002");
    let message = format!("{runtime}/messages/action");
    crate::put_node(
        &tx,
        &message,
        "message",
        &json!({"record":{"action":{"kind":"call","activation":"@helper--run--task","input":{}}}}),
    )?;
    crate::put_edge(&tx, &legacy, "message", &message, 0, &json!({}))?;
    tx.commit()?;
    Ok(())
}

/// Retain roots with the historical root relation, including missing conversation memberships.
pub fn participant_roots(path: &Path, runtime: &str) -> CwpdbResult<()> {
    let mut db = open(path, runtime)?;
    let tx = db.transaction()?;
    let root = format!("{runtime}/cwpdb");
    for index in 0..6 {
        let conversation = format!("{runtime}/conversations/{index:05}");
        crate::put_node(
            &tx,
            &conversation,
            "conversation",
            &json!({"creation_order":index,"title":format!("Conversation {index}"),"commit_revision":index+1}),
        )?;
        crate::put_edge(
            &tx,
            &root,
            "root_conversation",
            &conversation,
            index,
            &json!({}),
        )?;
    }
    let actor = format!("{runtime}/actors/person");
    for index in [1, 3, 5] {
        crate::put_edge(
            &tx,
            &format!("{runtime}/conversations/{index:05}"),
            "participant",
            &actor,
            0,
            &json!({}),
        )?;
    }
    crate::put_edge(
        &tx,
        &format!("{runtime}/conversations/00003"),
        "archived",
        &root,
        0,
        &json!({}),
    )?;
    tx.commit()?;
    Ok(())
}
