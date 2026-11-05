use std::{
    ffi::{c_char, CStr, CString},
    ptr,
    sync::{Arc, Barrier},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde_json::json;

use super::*;

fn statement(sql: &str, params: Vec<Value>) -> SqlStatement {
    SqlStatement {
        sql: sql.into(),
        params,
    }
}

fn query(store: &mut NativeStore, sql: &str, params: Vec<Value>) -> QueryResponse {
    store
        .query(&QueryRequest {
            sql: sql.into(),
            params,
            max_rows: None,
        })
        .unwrap()
}

#[test]
fn batch_rolls_back_every_statement_when_one_fails() {
    let mut store = NativeStore::open_in_memory().unwrap();
    store
        .execute_batch(&BatchRequest {
            statements: vec![statement(
                "CREATE TABLE samples (sample_id INTEGER PRIMARY KEY, value TEXT NOT NULL)",
                vec![],
            )],
        })
        .unwrap();
    let error = store
        .execute_batch(&BatchRequest {
            statements: vec![
                statement(
                    "INSERT INTO samples(value) VALUES (?1)",
                    vec![json!("first")],
                ),
                statement("INSERT INTO samples(value) VALUES (?1)", vec![Value::Null]),
            ],
        })
        .expect_err("the batch must fail as a unit");
    assert!(error.to_string().contains("NOT NULL"));
    assert_eq!(
        query(&mut store, "SELECT COUNT(*) FROM samples", vec![]).rows,
        vec![vec![json!(0)]]
    );
}

#[test]
fn comment_prefixed_transaction_control_cannot_commit_a_partial_batch() {
    let mut store = NativeStore::open_in_memory().unwrap();
    store
        .execute_batch(&BatchRequest {
            statements: vec![statement(
                "CREATE TABLE samples (value TEXT NOT NULL)",
                vec![],
            )],
        })
        .unwrap();

    assert!(store
        .execute_batch(&BatchRequest {
            statements: vec![
                statement("INSERT INTO samples(value) VALUES ('first')", vec![]),
                statement("/* bypass the lexical guard */ COMMIT", vec![]),
                statement("INSERT INTO samples(value) VALUES (NULL)", vec![]),
            ],
        })
        .is_err());

    assert_eq!(
        query(&mut store, "SELECT COUNT(*) FROM samples", vec![]).rows,
        vec![vec![json!(0)]]
    );
}

#[test]
fn retained_metric_rows_expire_without_creating_graph_reference_bindings() {
    let mut store = NativeStore::open_in_memory().unwrap();
    let response = store
        .execute_batch(&BatchRequest {
            statements: vec![
                statement(
                    "CREATE TABLE performance_samples (recorded_at_ms INTEGER NOT NULL, invocation_ref TEXT NOT NULL)",
                    vec![],
                ),
                statement(
                    "INSERT INTO performance_samples(recorded_at_ms, invocation_ref) VALUES (?1, ?2)",
                    vec![json!(1), json!("cwp://example/invocations/expired")],
                ),
                statement(
                    "INSERT INTO performance_samples(recorded_at_ms, invocation_ref) VALUES (?1, ?2)",
                    vec![json!(2), json!("cwp://example/invocations/current")],
                ),
                statement(
                    "DELETE FROM performance_samples WHERE recorded_at_ms < ?1",
                    vec![json!(2)],
                ),
            ],
        })
        .unwrap();
    assert_eq!(response.changes, vec![0, 1, 1, 1]);
    assert_eq!(
        query(
            &mut store,
            "SELECT invocation_ref FROM performance_samples ORDER BY recorded_at_ms",
            vec![],
        )
        .rows,
        vec![vec![json!("cwp://example/invocations/current")]]
    );
    assert_eq!(
        query(&mut store, "SELECT COUNT(*) FROM ref_bindings", vec![]).rows,
        vec![vec![json!(0)]]
    );
}

#[test]
fn database_budget_reports_when_an_existing_store_is_already_larger() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("store.sqlite3");
    let mut store = NativeStore::open(&path).unwrap();
    store
        .execute_batch(&BatchRequest {
            statements: vec![statement(
                "CREATE TABLE samples (value TEXT NOT NULL)",
                vec![],
            )],
        })
        .unwrap();
    let statements = (0..16)
        .map(|_| {
            statement(
                "INSERT INTO samples(value) VALUES (?1)",
                vec![json!("x".repeat(16 * 1024))],
            )
        })
        .collect();
    store.execute_batch(&BatchRequest { statements }).unwrap();
    assert!(!store.checkpoint_wal().unwrap().busy);

    let effective = store.set_max_database_bytes(4 * 1024).unwrap();
    assert!(
        effective > 4 * 1024,
        "SQLite must report that it could not lower the cap below existing pages"
    );
}

#[test]
fn read_only_open_never_creates_or_mutates_a_database() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("metrics.sqlite3");

    assert!(NativeStore::open_read_only(&path).is_err());
    assert!(!path.exists());

    let mut writer = NativeStore::open(&path).unwrap();
    writer
        .execute_batch(&BatchRequest {
            statements: vec![statement(
                "CREATE TABLE samples (value TEXT NOT NULL)",
                vec![],
            )],
        })
        .unwrap();
    drop(writer);

    let mut reader = NativeStore::open_read_only(&path).unwrap();
    assert_eq!(
        query(&mut reader, "SELECT COUNT(*) FROM samples", vec![]).rows,
        vec![vec![json!(0)]]
    );
    assert!(reader
        .execute_batch(&BatchRequest {
            statements: vec![statement(
                "INSERT INTO samples(value) VALUES ('blocked')",
                vec![]
            )],
        })
        .is_err());
}

#[test]
fn concurrent_first_opens_share_one_schema_initialization() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("shared.sqlite3");
    let barrier = Arc::new(Barrier::new(8));
    let opens = (0..8)
        .map(|_| {
            let barrier = barrier.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                barrier.wait();
                NativeStore::open(path)
            })
        })
        .collect::<Vec<_>>();
    for open in opens {
        drop(open.join().expect("opening thread must not panic").unwrap());
    }

    let mut store = NativeStore::open(&path).unwrap();
    assert_eq!(
        query(
            &mut store,
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('nodes','edges','ref_bindings')",
            vec![],
        )
        .rows,
        vec![vec![json!(3)]]
    );
}

#[test]
fn query_boundary_rejects_mutation() {
    let mut store = NativeStore::open_in_memory().unwrap();
    store
        .execute_batch(&BatchRequest {
            statements: vec![statement(
                "CREATE TABLE samples (value TEXT NOT NULL)",
                vec![],
            )],
        })
        .unwrap();
    assert!(store
        .query(&QueryRequest {
            sql: "INSERT INTO samples(value) VALUES ('not-a-query') RETURNING value".into(),
            params: vec![],
            max_rows: None,
        })
        .is_err());
    assert_eq!(
        query(&mut store, "SELECT COUNT(*) FROM samples", vec![]).rows,
        vec![vec![json!(0)]]
    );
    assert!(store
        .query(&QueryRequest {
            sql: "PRAGMA query_only=OFF".into(),
            params: vec![],
            max_rows: None,
        })
        .is_err());
    assert!(store
        .query(&QueryRequest {
            sql: "EXPLAIN PRAGMA foreign_keys=OFF".into(),
            params: vec![],
            max_rows: None,
        })
        .is_err());
    store
        .execute_batch(&BatchRequest {
            statements: vec![statement(
                "INSERT INTO samples(value) VALUES ('safe')",
                vec![],
            )],
        })
        .unwrap();
    assert_eq!(
        query(&mut store, "SELECT COUNT(*) FROM samples", vec![]).rows,
        vec![vec![json!(1)]]
    );
}

#[cfg(unix)]
#[test]
fn opening_a_store_does_not_change_an_existing_parent_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().join("caller-owned");
    std::fs::create_dir(&parent).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();

    NativeStore::open(parent.join("store.sqlite3")).unwrap();

    assert_eq!(
        std::fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[cfg(unix)]
#[test]
fn opening_a_store_makes_every_new_parent_directory_private() {
    let temporary = tempfile::tempdir().unwrap();
    let outer = temporary.path().join("new-parent");
    let inner = outer.join("nested-parent");

    NativeStore::open(inner.join("store.sqlite3")).unwrap();

    for directory in [outer, inner] {
        assert_eq!(
            std::fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[cfg(unix)]
#[test]
fn opening_a_store_keeps_database_and_sidecars_private() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("store.sqlite3");
    let mut store = NativeStore::open(&path).unwrap();
    store
        .execute_batch(&BatchRequest {
            statements: vec![statement(
                "CREATE TABLE samples (value TEXT NOT NULL)",
                vec![],
            )],
        })
        .unwrap();

    let sidecars = sqlite_sidecar_paths(&path).unwrap();
    for file in std::iter::once(&path).chain(sidecars.iter()) {
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn relative_database_paths_use_the_current_directory_as_parent() {
    assert_eq!(
        database_parent(Path::new("local.db")).unwrap(),
        Path::new(".")
    );
}

#[test]
fn opening_a_relative_path_creates_missing_parent_directories() {
    let temporary = tempfile::Builder::new()
        .prefix("cwpdb-relative-")
        .tempdir_in(".")
        .unwrap();
    let root = temporary.path().file_name().unwrap();
    let path = Path::new(root).join("missing-parent").join("store.sqlite3");

    let store = NativeStore::open(&path).unwrap();
    assert!(path.is_file());
    drop(store);
}

#[test]
fn ffi_uses_caller_freed_json_and_error_strings() {
    let temporary = tempfile::tempdir().unwrap();
    let path = CString::new(temporary.path().join("store.sqlite3").to_str().unwrap()).unwrap();
    let mut handle = ptr::null_mut();
    let mut error = ptr::null_mut();
    assert_eq!(
        // SAFETY: test inputs and output pointers satisfy the exported C ABI.
        unsafe { cwpdb_open(path.as_ptr(), &mut handle, &mut error) },
        FFI_OK
    );
    assert!(!handle.is_null());
    assert!(error.is_null());

    let request = CString::new(
        serde_json::to_string(&BatchRequest {
            statements: vec![statement(
                "CREATE TABLE samples (value TEXT NOT NULL)",
                vec![],
            )],
        })
        .unwrap(),
    )
    .unwrap();
    let mut response = ptr::null_mut();
    assert_eq!(
        // SAFETY: test inputs and output pointers satisfy the exported C ABI.
        unsafe { cwpdb_execute_batch(handle, request.as_ptr(), &mut response, &mut error) },
        FFI_OK
    );
    // SAFETY: cwpdb returned a live NUL-terminated string for this successful request.
    let response_text = unsafe { CStr::from_ptr(response) }.to_str().unwrap();
    assert_eq!(response_text, r#"{"changes":[0]}"#);
    // SAFETY: this library allocated the response and closing handle according to its ABI.
    unsafe {
        cwpdb_string_free(response);
        cwpdb_close(handle);
    }

    let malformed = CString::new("not json").unwrap();
    let mut failed_response: *mut c_char = ptr::null_mut();
    error = ptr::null_mut();
    assert_eq!(
        // SAFETY: a null handle is intentionally tested and output pointers are writable.
        unsafe {
            cwpdb_execute_batch(
                ptr::null_mut(),
                malformed.as_ptr(),
                &mut failed_response,
                &mut error,
            )
        },
        FFI_ERROR
    );
    assert!(failed_response.is_null());
    assert!(!error.is_null());
    // SAFETY: cwpdb returned this error string and the ABI assigns cleanup to the caller.
    unsafe { cwpdb_string_free(error) };
}

#[test]
fn ffi_batch_requires_a_response_output_before_committing() {
    let temporary = tempfile::tempdir().unwrap();
    let path = CString::new(temporary.path().join("store.sqlite3").to_str().unwrap()).unwrap();
    let mut handle = ptr::null_mut();
    let mut error = ptr::null_mut();
    assert_eq!(
        // SAFETY: test inputs and output pointers satisfy the exported C ABI.
        unsafe { cwpdb_open(path.as_ptr(), &mut handle, &mut error) },
        FFI_OK
    );

    let create_table = CString::new(
        serde_json::to_string(&BatchRequest {
            statements: vec![statement(
                "CREATE TABLE must_not_exist (value TEXT NOT NULL)",
                vec![],
            )],
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        // SAFETY: a null response output is intentionally tested; the error output is writable.
        unsafe { cwpdb_execute_batch(handle, create_table.as_ptr(), ptr::null_mut(), &mut error,) },
        FFI_ERROR
    );
    assert!(!error.is_null());
    // SAFETY: cwpdb returned this error string and the ABI assigns cleanup to the caller.
    unsafe { cwpdb_string_free(error) };

    let table_count = CString::new(
        serde_json::to_string(&QueryRequest {
            sql: "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'must_not_exist'"
                .into(),
            params: vec![],
            max_rows: None,
        })
        .unwrap(),
    )
    .unwrap();
    let mut response = ptr::null_mut();
    error = ptr::null_mut();
    assert_eq!(
        // SAFETY: test inputs and output pointers satisfy the exported C ABI.
        unsafe { cwpdb_query(handle, table_count.as_ptr(), &mut response, &mut error) },
        FFI_OK
    );
    // SAFETY: cwpdb returned a live NUL-terminated JSON response.
    let response_text = unsafe { CStr::from_ptr(response) }.to_str().unwrap();
    assert_eq!(
        serde_json::from_str::<QueryResponse>(response_text)
            .unwrap()
            .rows,
        vec![vec![json!(0)]]
    );
    // SAFETY: this library allocated the response and handle according to its ABI.
    unsafe {
        cwpdb_string_free(response);
        cwpdb_close(handle);
    }
}

#[test]
fn json_integers_are_lossless_or_rejected() {
    for value in [
        i64::MIN,
        i64::MAX,
        9_007_199_254_740_992,
        -9_007_199_254_740_992,
    ] {
        assert!(json_value(ValueRef::Integer(value)).is_err());
    }
    for value in [-9_007_199_254_740_991, 0, 9_007_199_254_740_991] {
        assert_eq!(json_value(ValueRef::Integer(value)).unwrap(), json!(value));
    }
}

#[test]
fn current_schema_reopens_while_another_writer_holds_the_lock() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.sqlite3");
    let first = NativeStore::open(&path).unwrap();
    let version: i64 = first
        .connection
        .query_row("PRAGMA schema_version", [], |r| r.get(0))
        .unwrap();
    first.connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    let second = NativeStore::open(&path).unwrap();
    let reopened_version: i64 = second
        .connection
        .query_row("PRAGMA schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, reopened_version);
    first.connection.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn upgrade_is_inert_when_current_and_repairs_missing_triggers_and_indexes() {
    let mut store = NativeStore::open_in_memory().unwrap();
    let version = |db: &Connection| {
        db.query_row::<i64, _, _>("PRAGMA schema_version", [], |r| r.get(0))
            .unwrap()
    };
    let before = version(&store.connection);
    let tx = store.connection.transaction().unwrap();
    upgrade(&tx).unwrap();
    tx.commit().unwrap();
    assert_eq!(version(&store.connection), before);
    store
        .connection
        .execute_batch(
            "DROP TRIGGER ref_bindings_no_durable_delete;
             DROP INDEX nodes_schedule_due;
             DROP INDEX nodes_schedule_terminal;",
        )
        .unwrap();
    assert!(!crate::schema_is_current(&store.connection).unwrap());
    ensure_graph_schema(&mut store.connection).unwrap();
    assert!(crate::schema_is_current(&store.connection).unwrap());
    for index in ["nodes_schedule_due", "nodes_schedule_terminal"] {
        let installed: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                [index],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(installed, 1, "missing restored index {index}");
    }
}

#[test]
fn ffi_disposable_budget_and_checkpoint_use_the_owned_response_channel() {
    let path = CString::new(":memory:").unwrap();
    let mut handle = ptr::null_mut();
    let mut error = ptr::null_mut();
    let mut response = ptr::null_mut();
    // SAFETY: all pointers are live outputs or caller-owned, nul-terminated input.
    unsafe {
        assert_eq!(cwpdb_open(path.as_ptr(), &mut handle, &mut error), FFI_OK);
        assert_eq!(
            cwpdb_set_max_database_bytes(handle, 1024 * 1024, ptr::null_mut(), &mut error),
            FFI_ERROR
        );
        assert!(!error.is_null());
        cwpdb_string_free(error);
        error = ptr::null_mut();
        assert_eq!(
            cwpdb_set_max_database_bytes(handle, 1024 * 1024, &mut response, &mut error),
            FFI_OK
        );
        let limit: u64 = serde_json::from_str(CStr::from_ptr(response).to_str().unwrap()).unwrap();
        assert_eq!(limit, 1024 * 1024);
        cwpdb_string_free(response);
        assert_eq!(
            cwpdb_checkpoint_wal(handle, &mut response, &mut error),
            FFI_OK
        );
        let checkpoint: Value =
            serde_json::from_str(CStr::from_ptr(response).to_str().unwrap()).unwrap();
        assert_eq!(checkpoint["busy"], false);
        cwpdb_string_free(response);
        cwpdb_close(handle);
        assert_eq!(
            cwpdb_checkpoint_wal(ptr::null_mut(), &mut response, &mut error),
            FFI_ERROR
        );
        assert!(response.is_null());
        cwpdb_string_free(error);
    }
}
