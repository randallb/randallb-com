//! Native retained-handle identity fences remain owned by cwpdb.
use super::*;
struct Allow;
impl crate::invocation::Authorizer for Allow {
    fn authorize(&self, _: &crate::invocation::Context, _: &str, _: bool) -> CwpdbResult<()> {
        Ok(())
    }
}
#[test]
fn replacing_an_open_datafile_fences_reads_and_commits_before_any_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.sqlite");
    let runtime = "cwp://fixture/runtime";
    let scope = Scope {
        runtime_root_ref: runtime.into(),
        context: crate::invocation::Context {
            workspace_ref: runtime.into(),
            caller_ref: runtime.into(),
        },
    };
    let options = OpenOptions {
        runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
            .into(),
        workspace_binding: None,
        timeline_rollout: None,
    };
    let mut store = NativeConversationStore::open(&path, scope.clone(), options).unwrap();
    let (revision, counter) = current_revision(&store.connection, &scope).unwrap();
    let replacement = dir.path().join("replacement.sqlite");
    let sentinel = b"fictional replacement; must not be opened or mutated";
    std::fs::write(&replacement, sentinel).unwrap();
    std::fs::rename(replacement, &path).unwrap();
    let reference = "cwp://fixture/conversation";
    let error = store.get(&scope, reference, &Allow).unwrap_err();
    assert!(error.to_string().contains("was replaced"));
    let selection = ReadSelectionRequest {
        scope: scope.clone(),
        expected_commit_revision: None,
        selection: ReadSelectionOperation::Position,
        deadline_unix_ms: i64::MAX,
        max_bytes: MAX_WIRE_BYTES,
    };
    let control = ReadControl {
        cancellation: ReadCancellation::default(),
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(10),
        vm_steps: None,
    };
    assert!(store
        .read_selection(&selection, &control, &Allow)
        .unwrap_err()
        .to_string()
        .contains("was replaced"));

    let mutation = Mutation {
        scope: scope.clone(),
        request_id: "replaced-path".into(),
        expected_revision: revision.clone(),
        changes: Changes {
            records: vec![WriteRecord {
                reference: reference.into(),
                kind: RecordKind::Conversation,
                properties: json!({}),
                owner_ref: None,
            }],
            ..Changes::default()
        },
        journal: None,
        authority_guards: vec![],
    };
    let error = store.commit(&mutation, &Allow).unwrap_err();
    assert!(error.to_string().contains("was replaced"));
    assert_eq!(std::fs::read(path).unwrap(), sentinel);
    assert_eq!(
        current_revision(&store.connection, &scope).unwrap(),
        (revision, counter)
    );
    assert!(crate::node_props(&store.connection, reference)
        .unwrap()
        .is_none());
}
