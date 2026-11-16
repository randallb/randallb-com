//! Physical fault injection for completion rollback compatibility tests.
use crate::CwpdbResult;
use std::path::Path;

/// Reject completed invocation writes through either physical write path.
/// Opens an existing database only; exposes no SQL, connection, or callback.
pub fn install_write_failure(path: &Path) -> CwpdbResult<()> {
    let mut db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let tx = db.transaction()?;
    tx.execute_batch(
        "CREATE TRIGGER fail_archive_completion BEFORE INSERT ON nodes
         WHEN NEW.kind='invocation' AND json_extract(NEW.props,'$.record.state')='completed'
         BEGIN SELECT RAISE(ABORT,'archive write failed'); END;
         CREATE TRIGGER fail_archive_completion_update BEFORE UPDATE ON nodes
         WHEN NEW.kind='invocation' AND json_extract(NEW.props,'$.record.state')='completed'
         BEGIN SELECT RAISE(ABORT,'archive write failed'); END;",
    )?;
    tx.commit()?;
    Ok(())
}

/// Remove both completion failure paths before retrying the same operation.
pub fn remove_write_failure(path: &Path) -> CwpdbResult<()> {
    let mut db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let tx = db.transaction()?;
    tx.execute_batch(
        "DROP TRIGGER fail_archive_completion; DROP TRIGGER fail_archive_completion_update;",
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn completion_failure_covers_insert_and_update_then_allows_retry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runtime.sqlite");
        let mut store = crate::NativeStore::open(&path).unwrap();
        let running = "cwp://fixture/running";
        let schema = "cwp://fixture/schema";
        store
            .create_entity_with_kind(
                running,
                "invocation",
                schema,
                &json!({"record":{"state":"running"}}),
            )
            .unwrap();
        install_write_failure(&path).unwrap();
        let completed = json!({"record":{"state":"completed"}});
        assert!(store
            .create_entity_with_kind("cwp://fixture/completed", "invocation", schema, &completed)
            .is_err());
        let db = rusqlite::Connection::open(&path).unwrap();
        let update = || {
            db.execute(
                "UPDATE nodes SET props=jsonb(?1) WHERE kind='invocation'",
                [completed.to_string()],
            )
        };
        assert!(update()
            .unwrap_err()
            .to_string()
            .contains("archive write failed"));
        assert_eq!(
            store.get_entity(running).unwrap().value.properties["record"]["state"],
            "running"
        );
        store
            .create_entity_with_kind(
                "cwp://fixture/pending",
                "invocation",
                schema,
                &json!({"record":{"state":"pending"}}),
            )
            .unwrap();
        store
            .create_entity("cwp://fixture/unrelated", schema, &completed)
            .unwrap();
        remove_write_failure(&path).unwrap();
        assert_eq!(update().unwrap(), 2);
        store
            .create_entity_with_kind("cwp://fixture/completed", "invocation", schema, &completed)
            .unwrap();
    }

    #[test]
    fn completion_failure_never_creates_a_missing_database() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing.sqlite");
        assert!(install_write_failure(&path).is_err());
        assert!(remove_write_failure(&path).is_err());
        assert!(!path.exists());
    }
}
