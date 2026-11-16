use super::*;
fn scope() -> Scope {
    Scope {
        context: Context {
            workspace_ref: "cwp://fixture/workspaces/open".into(),
            caller_ref: "cwp://fixture/actors/coordinator".into(),
        },
        runtime_root_ref: "cwp://fixture/runtimes/open".into(),
    }
}
fn options() -> OpenOptions {
    OpenOptions {
        runtime_state_version: STATE.into(),
        workspace_binding: None,
        timeline_rollout: None,
    }
}
fn bound_options(directory: &Path) -> OpenOptions {
    OpenOptions {
        workspace_binding: Some(WorkspaceBinding {
            workspace_id: "fictional-workspace-id".into(),
            workspace_directory: directory.into(),
        }),
        timeline_rollout: Some("new_conversations".into()),
        ..options()
    }
}
#[test]
fn unchanged_current_open_reads_while_another_writer_holds_immediate_lock() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.db");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let options = bound_options(&workspace);
    drop(NativeConversationStore::open(&path, scope(), options.clone()).unwrap());
    let writer = Connection::open(&path).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let store = NativeConversationStore::open(&path, scope(), options).unwrap();
    assert_eq!(
        store.connection.total_changes(),
        0,
        "unchanged open executed a mutation"
    );
    assert_eq!(
        runtime_metadata(&store.connection, &scope()).unwrap()["conversation_timeline_rollout"],
        "new_conversations"
    );
    writer.execute_batch("ROLLBACK").unwrap();
}
#[test]
fn unsupported_user_version_is_rejected_before_changing_database_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.db");
    drop(NativeConversationStore::open(&path, scope(), options()).unwrap());
    let raw = Connection::open(&path).unwrap();
    raw.pragma_update(None, "user_version", 17).unwrap();
    drop(raw);
    let before = std::fs::read(&path).unwrap();
    let error = NativeConversationStore::open(&path, scope(), options())
        .err()
        .expect("unsupported version was accepted");
    assert!(
        error
            .to_string()
            .contains("unsupported CWR runtime database schema version17")
            || error
                .to_string()
                .contains("unsupported CWR runtime database schema version 17")
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let raw = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        17
    );
}
#[test]
fn missing_legacy_cloud_binding_is_written_once_then_future_opens_are_readonly() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.db");
    drop(NativeConversationStore::open(&path, scope(), options()).unwrap());
    let raw = Connection::open(&path).unwrap();
    raw.execute(
        "UPDATE nodes SET props=jsonb_remove(props,'$.cloud_workspace_ref') WHERE kind='runtime'",
        [],
    )
    .unwrap();
    drop(raw);
    let store = NativeConversationStore::open(&path, scope(), options()).unwrap();
    assert!(store.connection.total_changes() > 0);
    assert_eq!(
        runtime_metadata(&store.connection, &scope()).unwrap()["cloud_workspace_ref"],
        scope().context.workspace_ref
    );
    drop(store);
    let store = NativeConversationStore::open(&path, scope(), options()).unwrap();
    assert_eq!(store.connection.total_changes(), 0);
}
#[test]
fn workspace_mismatch_fails_readonly_and_missing_original_directory_can_relocate_once() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.db");
    let old = temp.path().join("old");
    let new = temp.path().join("new");
    std::fs::create_dir(&old).unwrap();
    drop(NativeConversationStore::open(&path, scope(), bound_options(&old)).unwrap());
    let raw = Connection::open(&path).unwrap();
    raw.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut wrong = bound_options(&old);
    wrong.workspace_binding.as_mut().unwrap().workspace_id = "foreign-id".into();
    assert!(NativeConversationStore::open(&path, scope(), wrong)
        .err()
        .unwrap()
        .to_string()
        .contains("another workspace"));
    assert!(
        NativeConversationStore::open(&path, scope(), bound_options(&new))
            .err()
            .unwrap()
            .to_string()
            .contains("original directory")
    );
    raw.execute_batch("ROLLBACK").unwrap();
    drop(raw);
    std::fs::rename(&old, &new).unwrap();
    let moved = NativeConversationStore::open(&path, scope(), bound_options(&new)).unwrap();
    assert!(moved.connection.total_changes() > 0);
    drop(moved);
    let unchanged = NativeConversationStore::open(&path, scope(), bound_options(&new)).unwrap();
    assert_eq!(unchanged.connection.total_changes(), 0);
}
#[test]
fn invalid_requested_state_does_not_create_a_database_or_parent() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("not-created");
    let path = parent.join("runtime.db");
    let mut invalid = options();
    invalid.runtime_state_version = "cwp://fixture/unsupported-version".into();
    assert!(NativeConversationStore::open(&path, scope(), invalid).is_err());
    assert!(!path.exists());
    assert!(!parent.exists());
}
