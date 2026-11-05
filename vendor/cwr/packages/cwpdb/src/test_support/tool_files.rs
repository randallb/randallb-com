//! Fixed physical fixture for the canonical tool-files bootstrap reader.
use crate::{
    conversation::*,
    invocation::{Authorizer, Context},
    CwpdbResult, NativeStore,
};
use serde_json::json;
use std::path::Path;
struct Allow;
impl Authorizer for Allow {
    fn authorize(&self, _: &Context, _: &str, _: bool) -> CwpdbResult<()> {
        Ok(())
    }
}
/// Seed three fictional operations and deliberately out-of-order file edges,
/// including a file attached to the wrong owner kind. Refuse an existing graph.
/// Return the actual declared native bootstrap page, never a native handle.
pub fn bootstrap(path: &Path) -> CwpdbResult<SnapshotPage> {
    if path.exists() {
        return Err("tool-file fixture requires a new path".into());
    }
    let root = "cwp://example.test/runtime";
    let scope = Scope {
        runtime_root_ref: root.into(),
        context: Context {
            workspace_ref: root.into(),
            caller_ref: root.into(),
        },
    };
    let options = OpenOptions {
        runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
            .into(),
        workspace_binding: None,
        timeline_rollout: None,
    };
    let mut native = NativeConversationStore::open(path, scope.clone(), options)?;
    let mut raw = NativeStore::open(path)?;
    raw.graph_transaction(|tx|{
        for id in ["first","second","empty"] {
            let reference=format!("cwp://example.test/tool-operations/{id}");
            let operation=json!({"invocation_ref":"cwp://example.test/invocations/one","attempt_ref":"cwp://example.test/tool-attempts/one","provider":"test","provider_item_id":id,"host_call_id":null,"first_sequence":1,"sequence":1,"activity":{"activity_ref":reference,"kind":"command","label":id,"state":"completed","started_at_unix_ms":1,"child_invocation_ref":null,"duration_ms":null,"command":null,"output":null,"error":null,"exit_code":null,"files":[]}});
            crate::put_node(tx,&reference,"tool_operation",&json!({"record":operation}))?;
        }
        let unrelated="cwp://example.test/tool-attempts/unrelated";
        crate::put_node(tx,unrelated,"tool_attempt",&json!({}))?;
        for (owner,position,suffix,path) in [
            ("cwp://example.test/tool-operations/first",1,"first-1","a/after.rs"),
            ("cwp://example.test/tool-operations/second",0,"second-0","b/only.rs"),
            ("cwp://example.test/tool-operations/first",0,"first-0","a/before.rs"),
            (unrelated,0,"unrelated","wrong/owner.rs"),
        ]{
            let reference=format!("cwp://example.test/tool-files/{suffix}");
            crate::put_node(tx,&reference,"tool_file",&json!({"record":{"path":path,"kind":"modified","diff":null}}))?;
            crate::put_edge(tx,owner,"tool_file",&reference,position,&json!({}))?;
        }
        Ok(())
    })?;
    native.read(
        &ReadRequest {
            scope,
            selection: Selection::Bootstrap,
            continuation: None,
            page_size: 1000,
            max_bytes: MAX_WIRE_BYTES,
            expected_revision: None,
        },
        &Allow,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_bootstrap_excludes_wrong_owner_edges_and_fixture_refuses_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tools.sqlite");
        let page = bootstrap(&path).unwrap();
        assert!(page.complete);
        assert!(page.continuation.is_none());
        let files = page
            .relationships
            .iter()
            .filter(|r| r.kind == "tool_file")
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|r| r
            .source_ref
            .starts_with("cwp://example.test/tool-operations/")));
        assert!(page
            .records
            .iter()
            .any(|r| r.reference == "cwp://example.test/tool-files/unrelated"));
        assert!(bootstrap(&path).is_err());
    }
}
