//! Named retained-conversation fixtures. No caller-supplied SQL or native handle.
use crate::{CwpdbResult, NativeStore};
use rusqlite::OptionalExtension;
use serde_json::json;
use std::path::Path;

/// One legacy abandoned wait, retained independently from a host start-call identity.
/// Returns the new physical commit revision so a test resident can remain fenced.
pub fn abandoned_wait(
    path: &Path,
    runtime_root_ref: &str,
    waiter: &str,
    awaited: &str,
    call_id: &str,
) -> CwpdbResult<i64> {
    if call_id.is_empty() || call_id.len() > 4096 {
        return Err("invalid retained wait fixture call ID".into());
    }
    let mut store = NativeStore::open(path)?;
    store.graph_transaction(|tx| {
        let root = format!("{runtime_root_ref}/cwpdb");
        let metadata = crate::node_props(tx, &root)?.ok_or("fixture runtime missing")?;
        if metadata["runtime_root_ref"] != runtime_root_ref {
            return Err("fixture runtime identity mismatch".into());
        }
        for reference in [waiter, awaited] {
            let kind: Option<String> = tx.query_row("SELECT kind FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", [reference], |r| r.get(0)).optional()?;
            if kind.as_deref() != Some("invocation") {
                return Err("retained wait fixture requires existing invocations".into());
            }
        }
        let reference = crate::derived_ref("wait", &[waiter, call_id]);
        if crate::node_props(tx, &reference)?.is_some() {
            return Err("retained wait fixture already exists".into());
        }
        let record = json!({"waiter_invocation_ref":waiter,"awaited_invocation_ref":awaited,"call_id":call_id,"state":"abandoned"});
        crate::put_node(tx, &reference, "wait", &json!({"record":record}))?;
        for (field, target) in [("waiter_invocation_ref", waiter), ("awaited_invocation_ref", awaited)] {
            crate::put_edge(tx, &reference, &format!("field:{field}"), target, 0, &json!({}))?;
        }
        let next: i64 = tx.query_row("SELECT COALESCE(MAX(position),-1)+1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='wait'",[&root],|r|r.get(0))?;
        crate::put_edge(tx, &root, "wait", &reference, next, &json!({}))?;
        let revision = metadata["commit_revision"].as_i64().ok_or("fixture commit revision missing")?.checked_add(1).ok_or("fixture revision exhausted")?;
        crate::put_node(tx, &root, "runtime", &json!({"commit_revision":revision}))?;
        Ok(revision)
    })
}

/// Reproduce a legacy importer's header revision projection using the exact
/// current storage-owned runtime clock. The normal mutation port still rejects
/// caller-chosen clocks. This fixture refuses concurrent revision changes.
pub fn legacy_conversation_clocks(
    path: &Path,
    runtime_root_ref: &str,
    expected_revision: i64,
    references: &[String],
) -> CwpdbResult<()> {
    if references.len() > 50000 {
        return Err("fixture conversation work limit exceeded".into());
    }
    let mut store = NativeStore::open(path)?;
    store.graph_transaction(|tx|{
        let root=format!("{runtime_root_ref}/cwpdb");
        let metadata=crate::node_props(tx,&root)?.ok_or("fixture runtime missing")?;
        if metadata["runtime_root_ref"]!=runtime_root_ref || metadata["commit_revision"].as_i64()!=Some(expected_revision) {
            return Err("fixture runtime revision conflict".into());
        }
        for reference in references {
            let owned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes n ON n.ref_id=e.target_id AND n.kind='conversation' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='conversation' AND e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2))",rusqlite::params![root,reference],|r|r.get(0))?;
            if !owned {return Err("fixture conversation is not owned by runtime".into());}
        }
        for reference in references {crate::put_node(tx,reference,"conversation",&json!({"commit_revision":expected_revision}))?;}
        Ok(())
    })
}

/// Inject or remove the fixed tool-operation write failure used to verify rollback.
pub fn reject_tool_operation_writes(path: &Path, reject: bool) -> CwpdbResult<()> {
    let db = rusqlite::Connection::open(path)?;
    if reject {
        db.execute_batch("CREATE TRIGGER fail_tool BEFORE INSERT ON nodes WHEN NEW.kind='tool_operation' BEGIN SELECT RAISE(ABORT,'injected tool write failure'); END;")?;
    } else {
        db.execute_batch("DROP TRIGGER fail_tool;")?;
    }
    Ok(())
}
#[derive(Clone, Copy)]
pub enum InspectorRecordKind {
    Attempt,
    Operation,
    File,
}
impl InspectorRecordKind {
    fn storage_kind(self) -> &'static str {
        match self {
            Self::Attempt => "tool_attempt",
            Self::Operation => "tool_operation",
            Self::File => "tool_file",
        }
    }
}
/// Deliberately set an inspector record's schema to its own identity, preserving
/// every other byte of retained protocol data for reopen rejection assertions.
pub fn unsupported_inspector_schema(path: &Path, kind: InspectorRecordKind) -> CwpdbResult<usize> {
    let db = rusqlite::Connection::open(path)?;
    Ok(db.execute(
        "UPDATE nodes SET schema_id=ref_id WHERE kind=?1",
        [kind.storage_kind()],
    )?)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn retained_wait_fixture_checks_inputs_and_keeps_failed_changes_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.sqlite");
        let runtime = "cwp://fixture/runtime";
        let waiter = "cwp://fixture/waiter";
        let awaited = "cwp://fixture/awaited";
        let mut store = NativeStore::open(&path).unwrap();
        store
            .graph_transaction(|tx| {
                crate::put_node(
                    tx,
                    &format!("{runtime}/cwpdb"),
                    "runtime",
                    &json!({"runtime_root_ref":runtime,"commit_revision":4}),
                )?;
                crate::put_node(tx, waiter, "invocation", &json!({}))?;
                crate::put_node(tx, awaited, "invocation", &json!({}))?;
                Ok(())
            })
            .unwrap();
        assert!(abandoned_wait(&path, runtime, waiter, "cwp://fixture/missing", "wait").is_err());
        let mut read = |reference: &str| -> Value {
            store
                .graph_transaction(|tx| Ok(crate::node_props(tx, reference)?.unwrap()))
                .unwrap()
        };
        assert_eq!(read(&format!("{runtime}/cwpdb"))["commit_revision"], 4);
        assert_eq!(
            abandoned_wait(&path, runtime, waiter, awaited, "wait").unwrap(),
            5
        );
        assert!(abandoned_wait(&path, runtime, waiter, awaited, "wait").is_err());
        let reference = crate::derived_ref("wait", &[waiter, "wait"]);
        assert_eq!(read(&reference)["record"]["state"], "abandoned");
        assert_eq!(read(&format!("{runtime}/cwpdb"))["commit_revision"], 5);
    }
    #[test]
    fn legacy_clock_fixture_uses_current_runtime_revision_and_rejects_other_owners_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clock.sqlite");
        let runtime = "cwp://fixture/runtime";
        let conversation = "cwp://fixture/conversation";
        let mut store = NativeStore::open(&path).unwrap();
        store
            .graph_transaction(|tx| {
                crate::put_node(
                    tx,
                    &format!("{runtime}/cwpdb"),
                    "runtime",
                    &json!({"runtime_root_ref":runtime,"commit_revision":9}),
                )?;
                crate::put_node(
                    tx,
                    conversation,
                    "conversation",
                    &json!({"commit_revision":1}),
                )?;
                crate::put_edge(
                    tx,
                    &format!("{runtime}/cwpdb"),
                    "conversation",
                    conversation,
                    0,
                    &json!({}),
                )?;
                Ok(())
            })
            .unwrap();
        assert!(legacy_conversation_clocks(&path, runtime, 8, &[conversation.into()]).is_err());
        assert!(legacy_conversation_clocks(
            &path,
            runtime,
            9,
            &[conversation.into(), "cwp://fixture/foreign".into()]
        )
        .is_err());
        store
            .graph_transaction(|tx| {
                assert_eq!(
                    crate::node_props(tx, conversation)?.unwrap()["commit_revision"],
                    1
                );
                Ok(())
            })
            .unwrap();
        legacy_conversation_clocks(&path, runtime, 9, &[conversation.into()]).unwrap();
        store
            .graph_transaction(|tx| {
                assert_eq!(
                    crate::node_props(tx, conversation)?.unwrap()["commit_revision"],
                    9
                );
                assert_eq!(
                    crate::node_props(tx, &format!("{runtime}/cwpdb"))?.unwrap()["commit_revision"],
                    9
                );
                Ok(())
            })
            .unwrap();
    }
}
