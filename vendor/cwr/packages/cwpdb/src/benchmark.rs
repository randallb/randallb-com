//! Finite physical inspection for the provider-free CWR runtime benchmark.
//! Available only through the explicit development test-support feature.
use crate::CwpdbResult;
use std::path::Path;

pub struct RuntimeDatabaseSnapshot {
    pub sqlite_version: String,
    pub database_bytes: u64,
}

/// Checkpoint and verify one existing benchmark database. Never creates a file,
/// changes its schema, returns a native handle or accepts caller-supplied SQL.
pub fn checkpoint_runtime_database(path: &Path) -> CwpdbResult<RuntimeDatabaseSnapshot> {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    let integrity: String = db.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err("runtime benchmark database integrity check failed".into());
    }
    Ok(RuntimeDatabaseSnapshot {
        sqlite_version: rusqlite::version().to_owned(),
        database_bytes: std::fs::metadata(path)?.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn benchmark_checkpoint_reports_existing_graph_and_preserves_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("benchmark.sqlite3");
        let mut store = crate::NativeStore::open(&path).unwrap();
        store
            .create_entity(
                "cwp://fixture/kept",
                "cwp://fixture/schema",
                &serde_json::json!({"value":"kept"}),
            )
            .unwrap();
        drop(store);
        let report = checkpoint_runtime_database(&path).unwrap();
        assert_eq!(report.sqlite_version, rusqlite::version());
        assert_eq!(
            report.database_bytes,
            std::fs::metadata(&path).unwrap().len()
        );
        assert!(report.database_bytes > 0);
        let store = crate::NativeStore::open_read_only(&path).unwrap();
        assert_eq!(
            store
                .get_entity("cwp://fixture/kept")
                .unwrap()
                .value
                .properties["value"],
            "kept"
        );
    }
    #[test]
    fn benchmark_checkpoint_refuses_to_create_missing_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.sqlite3");
        assert!(checkpoint_runtime_database(&path).is_err());
        assert!(!path.exists());
    }
}
