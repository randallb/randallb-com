#[test]
fn legacy_json_is_not_imported_or_replaced() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let state_path = temporary.path().join("conversation.json");
    let legacy = br#"{
  "version": "cwr.conversation-admission/1",
  "runtime_root_ref": "cwp://com.example.runtime",
  "conversations": [],
  "admissions": []
}"#;
    std::fs::write(&state_path, legacy).unwrap();

    let error = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
        .err()
        .expect("legacy JSON must not be imported as SQLite");

    assert!(error.to_string().contains("runtime database error"));
    assert_eq!(std::fs::read(state_path).unwrap(), legacy);
}

#[test]
fn unsupported_runtime_database_schema_is_not_replaced() {
    use std::os::unix::fs::PermissionsExt as _;

    let temporary = tempfile::tempdir().expect("temporary state");
    let state_path = temporary.path().join("cwpdb.sqlite3");
    cwpdb_store::test_support::timeline_persistence::seed_unsupported_user_version(&state_path).unwrap();
    std::fs::set_permissions(&state_path, std::fs::Permissions::from_mode(0o640)).unwrap();

    let before = std::fs::read(&state_path).unwrap();
    let error = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
        .err()
        .expect("unsupported schema must fail closed");

    assert!(error
        .to_string()
        .contains("unsupported CWR runtime database schema version 99"));
    assert_eq!(std::fs::read(&state_path).unwrap(), before);
    assert_eq!(
        std::fs::metadata(&state_path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert!(!state_path.with_extension("sqlite3-wal").exists());
}

#[test]
fn non_utf8_runtime_database_path_is_rejected_before_open() {
    use std::os::unix::ffi::OsStringExt as _;

    let temporary = tempfile::tempdir().expect("temporary state");
    let state_path = temporary.path().join(std::ffi::OsString::from_vec(
        b"runtime-\xff.sqlite3".to_vec(),
    ));
    let error = ConversationAdmissionStore::open(&state_path, "cwp://com.example.runtime")
        .err()
        .expect("SQLite paths must be valid UTF-8");

    assert!(error
        .to_string()
        .contains("runtime database path must be valid UTF-8"));
    assert!(!state_path.exists());
}

#[cfg(unix)]
#[test]
fn native_backend_rejects_replaced_database_without_advancing_protocol_state() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://example.test/runtime").unwrap();
    let exchange = store
        .record_host_exchange(
            None,
            "cwp://example.test/person",
            "cwp://example.test/helper",
            "Question".into(),
            "Answer".into(),
        )
        .unwrap();
    let before = serde_json::to_value(&store.state).unwrap();
    let replacement = directory.path().join("replacement.sqlite");
    let sentinel = b"fictional replacement datafile; must stay unchanged";
    std::fs::write(&replacement, sentinel).unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    let error = store
        .record_host_exchange(
            Some(&exchange.conversation.conversation_ref),
            "cwp://example.test/person",
            "cwp://example.test/helper",
            "Another question".into(),
            "Another answer".into(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("was replaced"), "{error}");
    assert_eq!(serde_json::to_value(&store.state).unwrap(), before);
    assert_eq!(std::fs::read(&path).unwrap(), sentinel);
}
