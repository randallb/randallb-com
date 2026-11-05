//! Retained physical-schema assertions moved from CWR; protocol/schema
//! validation remains in CWR and does not expose a native handle.
use super::*;
use rusqlite::Connection;
struct State {
    runtime_root_ref: String,
}
fn open_database(
    path: &Path,
    root: &str,
    binding: Option<(&str, &Path)>,
) -> CwpdbResult<(Connection, (), State, i64)> {
    open_with_options(path, root, binding, None)
}
fn open_database_with_timeline_rollout(
    path: &Path,
    root: &str,
    binding: Option<(&str, &Path)>,
    stage: &str,
) -> CwpdbResult<(Connection, (), State, i64)> {
    open_with_options(path, root, binding, Some(stage.into()))
}
fn open_with_options(
    path: &Path,
    root: &str,
    binding: Option<(&str, &Path)>,
    timeline_rollout: Option<String>,
) -> CwpdbResult<(Connection, (), State, i64)> {
    let scope = Scope {
        runtime_root_ref: root.into(),
        context: crate::invocation::Context {
            workspace_ref: root.into(),
            caller_ref: root.into(),
        },
    };
    let mut store = NativeConversationStore::open(
        path,
        scope.clone(),
        OpenOptions {
            runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
                .into(),
            workspace_binding: binding.map(|(id, path)| WorkspaceBinding {
                workspace_id: id.into(),
                workspace_directory: path.into(),
            }),
            timeline_rollout,
        },
    )?;
    let metadata = runtime_metadata(&store.connection, &scope)?;
    let state = State {
        runtime_root_ref: metadata["runtime_root_ref"]
            .as_str()
            .ok_or("missing runtime root")?
            .into(),
    };
    let revision = metadata["commit_revision"]
        .as_i64()
        .ok_or("missing commit revision")?;
    Ok((
        store.connection.connection.take().unwrap(),
        (),
        state,
        revision,
    ))
}
#[test]
fn opening_a_current_runtime_does_not_require_a_write_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite3");
    let root = "cwp://example.test/runtime";
    let (writer, _, _, _) = open_database(&path, root, None).unwrap();
    let version: i64 = writer
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();

    let (reader, _, state, _) = open_database(&path, root, None).unwrap();
    assert_eq!(state.runtime_root_ref, root);
    let reopened_version: i64 = reader
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(reopened_version, version);
    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn unchanged_workspace_and_rollout_do_not_require_a_write_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite3");
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let root = "cwp://example.test/runtime";
    let binding = Some(("workspace-one", workspace.as_path()));
    let rollout = "new_conversations";
    let (writer, _, _, _) =
        open_database_with_timeline_rollout(&path, root, binding, rollout).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();

    let (_, _, state, _) =
        open_database_with_timeline_rollout(&path, root, binding, rollout).unwrap();
    assert_eq!(state.runtime_root_ref, root);
    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn opening_an_existing_runtime_does_not_upgrade_old_bindings() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite3");
    let root = "cwp://example.test/runtime";
    let (connection, _, _, _) = open_database(&path, root, None).unwrap();
    drop(connection);

    let legacy = Connection::open(&path).unwrap();
    legacy
        .execute_batch(
            r#"
                DROP TRIGGER ref_bindings_no_canonical_update;
                DROP TRIGGER ref_bindings_no_ref_id_update;
                DROP TRIGGER ref_bindings_no_disposable_downgrade;
                DROP TRIGGER ref_bindings_no_durable_delete;
                DROP INDEX ref_bindings_disposable;
                ALTER TABLE ref_bindings DROP COLUMN disposable;
                CREATE TRIGGER ref_bindings_no_update BEFORE UPDATE ON ref_bindings
                BEGIN SELECT RAISE(ABORT, 'reference bindings are immutable'); END;
                CREATE TRIGGER ref_bindings_no_delete BEFORE DELETE ON ref_bindings
                BEGIN SELECT RAISE(ABORT, 'reference bindings are immutable'); END;
            "#,
        )
        .unwrap();
    drop(legacy);

    let before = std::fs::read(&path).unwrap();
    let error = open_database(&path, root, None)
        .err()
        .expect("the old physical schema must need explicit conversion");
    assert!(error
        .to_string()
        .contains("unsupported cwpDb physical schema"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn opening_a_store_with_the_known_legacy_schedule_index_pair_does_not_rebuild_it() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite3");
    let root = "cwp://example.test/runtime";
    let (connection, _, _, _) = open_database(&path, root, None).unwrap();
    connection
        .execute_batch("DROP INDEX nodes_schedule_due; DROP INDEX nodes_schedule_terminal;")
        .unwrap();

    // The large foo fixture has this exact known catalog. Opening it must
    // leave the missing accelerators absent instead of scanning old history.
    let before_version: i64 = connection
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .unwrap();
    drop(connection);
    let (reopened, _, _, _) = open_database(&path, root, None).unwrap();
    let after_version: i64 = reopened
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(after_version, before_version);
    let schedule_indexes: i64 = reopened
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('nodes_schedule_due','nodes_schedule_terminal')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(schedule_indexes, 0);
}

#[test]
fn opening_a_store_with_only_one_missing_schedule_index_requires_conversion() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite3");
    let root = "cwp://example.test/runtime";
    let (connection, _, _, _) = open_database(&path, root, None).unwrap();
    connection
        .execute_batch("DROP INDEX nodes_schedule_due;")
        .unwrap();
    drop(connection);

    let error = open_database(&path, root, None).err().unwrap();
    assert!(error
        .to_string()
        .contains("missing index nodes_schedule_due"));
}

#[test]
fn opening_a_store_with_a_changed_index_definition_does_not_rewrite_it() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite3");
    let root = "cwp://example.test/runtime";
    let (connection, _, _, _) = open_database(&path, root, None).unwrap();
    connection
        .execute_batch(
            "DROP INDEX nodes_schedule_due;
             CREATE INDEX nodes_schedule_due ON nodes(kind);",
        )
        .unwrap();
    drop(connection);

    let before = std::fs::read(&path).unwrap();
    let error = open_database(&path, root, None).err().unwrap();
    assert!(error
        .to_string()
        .contains("different definition for index nodes_schedule_due"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn opening_a_store_with_a_changed_identity_trigger_does_not_rewrite_it() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite3");
    let root = "cwp://example.test/runtime";
    let (connection, _, _, _) = open_database(&path, root, None).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER ref_bindings_no_claim_update;
             CREATE TRIGGER ref_bindings_no_claim_update
             BEFORE UPDATE OF claim_kind, claim_owner_ref ON ref_bindings
             BEGIN SELECT 1; END;",
        )
        .unwrap();
    drop(connection);

    let before = std::fs::read(&path).unwrap();
    let error = open_database(&path, root, None).err().unwrap();
    assert!(error
        .to_string()
        .contains("different definition for trigger ref_bindings_no_claim_update"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn opening_a_database_with_a_view_does_not_initialize_it() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite3");
    let root = "cwp://example.test/runtime";
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE VIEW sqlitex AS SELECT 1;")
        .unwrap();
    drop(connection);

    let before = std::fs::read(&path).unwrap();
    let error = open_database(&path, root, None)
        .err()
        .expect("a populated schema is not a new database");
    assert!(error.to_string().contains("unexpected view sqlitex"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn opening_a_current_database_with_a_shadowed_extra_view_rejects_it() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite3");
    let root = "cwp://example.test/runtime";
    let (connection, _, _, _) = open_database(&path, root, None).unwrap();
    connection
        .execute_batch("CREATE VIEW sqlitex AS SELECT 1;")
        .unwrap();
    drop(connection);

    let error = open_database(&path, root, None).err().unwrap();
    assert!(error.to_string().contains("unexpected view sqlitex"));
}

#[test]
fn opening_a_database_with_only_sqlite_sequence_does_not_initialize_it() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite3");
    let root = "cwp://example.test/runtime";
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE temporary_record(id INTEGER PRIMARY KEY AUTOINCREMENT);
             DROP TABLE temporary_record;",
        )
        .unwrap();
    let remaining: String = connection
        .query_row("SELECT name FROM sqlite_master", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, "sqlite_sequence");
    drop(connection);

    let before = std::fs::read(&path).unwrap();
    let error = open_database(&path, root, None).err().unwrap();
    assert!(error
        .to_string()
        .contains("unexpected table sqlite_sequence"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn opening_reads_state_and_commit_revision_from_one_snapshot() {
    struct Permit;
    impl Authorizer for Permit {
        fn authorize(&self, _: &Context, _: &str, _: bool) -> CwpdbResult<()> {
            Ok(())
        }
    }
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("snapshot-open.sqlite3");
    let root = "cwp://example.test/runtime";
    let scope = Scope {
        runtime_root_ref: root.into(),
        context: Context {
            workspace_ref: root.into(),
            caller_ref: root.into(),
        },
    };
    let options = || OpenOptions {
        runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
            .into(),
        workspace_binding: None,
        timeline_rollout: None,
    };
    let mut writer = NativeConversationStore::open(&path, scope.clone(), options()).unwrap();
    let commit = |writer: &mut NativeConversationStore, reference: &str, id: &str| {
        let (revision, _) = current_revision(&writer.connection, &scope).unwrap();
        writer
            .commit(
                &Mutation {
                    scope: scope.clone(),
                    request_id: id.into(),
                    expected_revision: revision,
                    changes: Changes {
                        records: vec![WriteRecord {
                            reference: reference.into(),
                            kind: RecordKind::Conversation,
                            properties: json!({"title":id}),
                            owner_ref: None,
                        }],
                        memberships: vec![MembershipChange::Upsert {
                            source_ref: root_ref(&scope),
                            kind: MembershipKind::Conversation,
                            member: Member {
                                target_ref: reference.into(),
                                position: 0,
                                properties: json!({}),
                            },
                        }],
                        ..Default::default()
                    },
                    journal: None,
                    authority_guards: vec![],
                },
                &Permit,
            )
            .unwrap();
    };
    commit(
        &mut writer,
        "cwp://example.test/conversation/first",
        "first",
    );
    let mut reader = NativeConversationStore::open(&path, scope.clone(), options()).unwrap();
    let transaction = reader.connection.transaction().unwrap();
    let (_, expected_revision) = current_revision(&transaction, &scope).unwrap();
    let expected_conversation_count: i64 = transaction
        .query_row(
            "SELECT count(*) FROM nodes WHERE kind='conversation'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Establish the reader's WAL snapshot before the independent writer commits.
    commit(
        &mut writer,
        "cwp://example.test/conversation/concurrent",
        "concurrent",
    );
    let snapshot_revision = runtime_metadata(&transaction, &scope).unwrap()["commit_revision"]
        .as_i64()
        .unwrap();
    let snapshot_conversation_count: i64 = transaction
        .query_row(
            "SELECT count(*) FROM nodes WHERE kind='conversation'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(snapshot_conversation_count, expected_conversation_count);
    assert_eq!(snapshot_revision, expected_revision);
    transaction.commit().unwrap();
    assert!(current_revision(&reader.connection, &scope).unwrap().1 > snapshot_revision);
}
