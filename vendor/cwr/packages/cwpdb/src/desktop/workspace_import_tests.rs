use crate::NativeStore;
fn execute(store: &mut NativeStore, source: &str) -> Result<Response> {
    super::execute(
        store,
        &super::super::test_catalog().map_err(storage)?,
        source,
    )
}
fn test_execute_with_probe(
    store: &mut NativeStore,
    source: &str,
    probe: impl FnOnce(),
) -> Result<Response> {
    super::execute_with_probe(
        store,
        &super::super::test_catalog().map_err(storage)?,
        source,
        probe,
    )
}
use super::*;
use crate::{BatchRequest, SqlStatement};
fn source(path: &Path) -> crate::CwpdbResult<()> {
    let mut store = NativeStore::open(path)?;
    store.execute_batch(&BatchRequest{statements:vec![
        SqlStatement{sql:"CREATE TABLE blank_loop_instances_v1(workspace TEXT PRIMARY KEY,id TEXT,name TEXT,folders TEXT)".into(),params:vec![]},
        SqlStatement{sql:"CREATE TABLE blank_loop_versions_v1(workspace TEXT,version TEXT,files TEXT,folders TEXT)".into(),params:vec![]},
        SqlStatement{sql:"CREATE TABLE blank_loop_active_v1(workspace TEXT,version TEXT)".into(),params:vec![]},
        SqlStatement{sql:"INSERT INTO blank_loop_instances_v1 VALUES('/scope','fixture','Retained','[]')".into(),params:vec![]},
        SqlStatement{sql:"INSERT INTO blank_loop_versions_v1 VALUES('/scope','one','[]','[\"/approved\"]')".into(),params:vec![]},
        SqlStatement{sql:"INSERT INTO blank_loop_active_v1 VALUES('/scope','one')".into(),params:vec![]},
    ]})?;
    Ok(())
}
fn marker(source: &str) -> String {
    cwpdb::unordered::desktop_ref("storage-import", &json!(source))
}
#[test]
fn missing_disappearing_and_empty_replaced_sources_never_create_false_completion(
) -> crate::CwpdbResult<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("legacy.db");
    let source_path = path.to_str().unwrap();
    let mut destination = NativeStore::open_in_memory()?;
    assert!(matches!(
        execute(&mut destination, source_path)?,
        Response::NoSource
    ));
    source(&path)?;
    let saved = directory.path().join("saved.db");
    let result = test_execute_with_probe(&mut destination, source_path, || {
        fs::rename(&path, &saved).unwrap()
    });
    assert!(result.is_err());
    assert!(!path.exists());
    destination.graph_read(|db| {
        assert!(!cwpdb::record_exists(db, &marker(source_path))?);
        Ok(())
    })?;
    // A zero-table replacement cannot suppress the real retained source.
    fs::write(&path, [])?;
    assert!(execute(&mut destination, source_path).is_err());
    fs::remove_file(&path)?;
    fs::rename(&saved, &path)?;
    assert!(matches!(
        execute(&mut destination, source_path)?,
        Response::Complete { .. }
    ));
    Ok(())
}
#[test]
fn import_failure_is_all_or_nothing_and_completion_stops_legacy_reads() -> crate::CwpdbResult<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("legacy.db");
    source(&path)?;
    let source_path = path.to_str().unwrap();
    let mut destination = NativeStore::open_in_memory()?;
    destination.graph_transaction(|tx|{tx.execute_batch("CREATE TRIGGER inject_import BEFORE INSERT ON nodes WHEN NEW.kind='desktop_version_membership_v1' BEGIN SELECT RAISE(ABORT,'injected'); END")?;Ok(())})?;
    assert!(execute(&mut destination, source_path).is_err());
    destination.graph_read(|db| {
        let count: i64 = db.query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))?;
        assert_eq!(count, 0);
        Ok(())
    })?;
    destination.graph_transaction(|tx| {
        tx.execute_batch("DROP TRIGGER inject_import")?;
        Ok(())
    })?;
    execute(&mut destination, source_path)?;
    fs::remove_file(&path)?;
    assert!(matches!(
        execute(&mut destination, source_path)?,
        Response::AlreadyComplete
    ));
    let state = versions::execute(
        &mut destination,
        &super::super::test_catalog()?,
        versions::Request::ReadInstance {
            root: "/scope".into(),
        },
    )?;
    assert!(matches!(
        state,
        versions::Response::Instance {
            membership_ready: true,
            ..
        }
    ));
    Ok(())
}
#[test]
fn old_marker_without_source_evidence_does_not_suppress_later_restoration() -> crate::CwpdbResult<()>
{
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("legacy.db");
    let source_path = path.to_str().unwrap();
    let mut destination = NativeStore::open_in_memory()?;
    destination.graph_transaction(|tx| {
        cwpdb::put_node_with_schema(
            tx,
            &marker(source_path),
            "desktop_storage_import_v1",
            "cwp://org.coworkerprotocol/cwpdb/schemas/desktop_storage_import_v1/2026-09",
            &json!({"record":{"source":source_path,"complete":true}}),
        )?;
        Ok(())
    })?;
    assert!(matches!(
        execute(&mut destination, source_path)?,
        Response::NoSource
    ));
    source(&path)?;
    assert!(matches!(
        execute(&mut destination, source_path)?,
        Response::Complete { .. }
    ));
    Ok(())
}
