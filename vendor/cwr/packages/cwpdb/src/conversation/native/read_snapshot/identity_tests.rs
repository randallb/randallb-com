use super::*;
const ROOT: &str = "cwp://fixture/runtime";
fn scope() -> Scope {
    Scope {
        runtime_root_ref: ROOT.into(),
        context: Context {
            workspace_ref: ROOT.into(),
            caller_ref: ROOT.into(),
        },
    }
}
fn options() -> OpenOptions {
    OpenOptions {
        runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
            .into(),
        workspace_binding: None,
        timeline_rollout: None,
    }
}
fn control() -> ReadControl {
    ReadControl {
        cancellation: ReadCancellation::default(),
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
        vm_steps: None,
    }
}
#[test]
fn unrelated_runtime_node_cannot_supply_expected_root_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("unowned.sqlite");
    let mut db = Connection::open(&path).unwrap();
    let tx = db.transaction().unwrap();
    crate::initialize(&tx).unwrap();
    crate::put_node(&tx,"cwp://unowned/runtime","runtime",&json!({"schema_ref":crate::SCHEMA_REF,"runtime_root_ref":ROOT,"runtime_state_version":options().runtime_state_version,"commit_revision":0})).unwrap();
    tx.commit().unwrap();
    assert!(NativeReadStore::open(&path, ROOT, &control()).is_err());
}
#[test]
fn unrelated_runtime_kind_node_makes_identity_ambiguous() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ambiguous.sqlite");
    let mut store = NativeConversationStore::open(&path, scope(), options()).unwrap();
    let tx = store.connection.transaction().unwrap();
    crate::put_node(
        &tx,
        "cwp://unowned/runtime",
        "runtime",
        &json!({"runtime_root_ref":"cwp://unowned"}),
    )
    .unwrap();
    tx.commit().unwrap();
    assert!(NativeReadStore::open(&path, ROOT, &control())
        .err()
        .unwrap()
        .to_string()
        .contains("ambiguous"));
}
#[cfg(unix)]
#[test]
fn prepared_factory_fences_replacement_before_new_selection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("replaced.sqlite");
    let _store = NativeConversationStore::open(&path, scope(), options()).unwrap();
    let mut factory = NativeReadStore::open(&path, ROOT, &control()).unwrap();
    let frozen = factory.begin_current(control()).unwrap();
    let replacement = directory.path().join("replacement.sqlite");
    let bytes = b"replacement must remain untouched";
    std::fs::write(&replacement, bytes).unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    assert!(!frozen
        .conversation_exists("cwp://missing/conversation", false, None)
        .unwrap());
    drop(frozen);
    assert!(factory
        .begin_current(control())
        .err()
        .unwrap()
        .to_string()
        .contains("was replaced"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}
