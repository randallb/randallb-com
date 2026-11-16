use super::*;
#[test]
fn runtime_database_uses_the_private_versioned_wal_schema() {
    use std::os::unix::fs::PermissionsExt as _;

    let temporary = tempfile::tempdir().expect("temporary state");
    let state_path = temporary.path().join("cwpdb.sqlite3");
    let root = "cwp://com.example.runtime";
    let scope = Scope {
        runtime_root_ref: root.into(),
        context: Context {
            workspace_ref: root.into(),
            caller_ref: root.into(),
        },
    };
    let store = NativeConversationStore::open(
        &state_path,
        scope,
        OpenOptions {
            runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
                .into(),
            workspace_binding: None,
            timeline_rollout: None,
        },
    )
    .unwrap();

    let journal_mode: String = store
        .connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let foreign_keys: i64 = store
        .connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    let synchronous: i64 = store
        .connection
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .unwrap();
    let schema_version: i64 = store
        .connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();

    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    assert_eq!(foreign_keys, 1);
    assert_eq!(synchronous, 2);
    assert_eq!(schema_version, 0);
    let props = crate::node_props(&store.connection, "cwp://com.example.runtime/cwpdb")
        .unwrap()
        .unwrap();
    assert_eq!(props["schema_ref"], crate::SCHEMA_REF);
    let tables: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 3);
    assert_eq!(
        std::fs::metadata(state_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
