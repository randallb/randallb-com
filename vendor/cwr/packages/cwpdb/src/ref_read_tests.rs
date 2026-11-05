use super::*;
use crate::{bind, derived_ref, initialize, put_edge, put_node, put_node_with_schema, remove};
use serde_json::json;

const REF: &str = "cwp://example/resources/one";
const SCHEMA: &str = "cwp://example/schemas/example";

fn initialize_db(db: &mut Connection) {
    let tx = db.transaction().unwrap();
    initialize(&tx).unwrap();
    tx.commit().unwrap();
}

#[test]
fn reads_committed_values_and_revisions_across_connections_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.sqlite");
    let mut writer = Connection::open(&path).unwrap();
    initialize_db(&mut writer);
    let reader = Connection::open(&path).unwrap();
    assert!(matches!(get_ref(&reader, REF), Err(RefReadError::NotFound)));
    let tx = writer.transaction().unwrap();
    put_node_with_schema(
        &tx,
        REF,
        "example",
        SCHEMA,
        &json!({"record":{"name":"one"}}),
    )
    .unwrap();
    tx.commit().unwrap();
    let first = get_ref(&reader, REF).unwrap();
    assert_eq!(first.reference, REF);
    assert_eq!(first.kind, "example");
    assert_eq!(first.value.schema_ref, SCHEMA);
    assert_eq!(first.value.properties, json!({"record":{"name":"one"}}));
    let tx = writer.transaction().unwrap();
    put_node_with_schema(
        &tx,
        REF,
        "example",
        SCHEMA,
        &json!({"record":{"name":"two"}}),
    )
    .unwrap();
    tx.commit().unwrap();
    let second = get_ref(&reader, REF).unwrap();
    assert_ne!(first.revision, second.revision);
    assert_eq!(second.value.properties, json!({"record":{"name":"two"}}));
    drop(reader);
    drop(writer);
    let reopened = Connection::open(&path).unwrap();
    assert_eq!(get_ref(&reopened, REF).unwrap(), second);
}

#[test]
fn missing_binding_deleted_record_and_edge_have_distinct_outcomes() {
    let mut db = Connection::open_in_memory().unwrap();
    initialize_db(&mut db);
    let count: i64 = db
        .query_row("SELECT count(*) FROM ref_bindings", [], |r| r.get(0))
        .unwrap();
    assert!(matches!(get_ref(&db, REF), Err(RefReadError::NotFound)));
    assert_eq!(
        db.query_row("SELECT count(*) FROM ref_bindings", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        count
    );
    let tx = db.transaction().unwrap();
    bind(&tx, REF).unwrap();
    tx.commit().unwrap();
    assert!(matches!(get_ref(&db, REF), Err(RefReadError::NotFound)));
    let tx = db.transaction().unwrap();
    put_node(&tx, REF, "example", &json!({})).unwrap();
    put_edge(&tx, REF, "related", "cwp://example/external", 0, &json!({})).unwrap();
    tx.commit().unwrap();
    let edge = derived_ref("related", &[REF, "cwp://example/external"]);
    assert!(matches!(
        get_ref(&db, &edge),
        Err(RefReadError::WrongRecordType)
    ));
    let tx = db.transaction().unwrap();
    remove(&tx, REF).unwrap();
    tx.commit().unwrap();
    assert!(matches!(get_ref(&db, REF), Err(RefReadError::NotFound)));
}

#[test]
fn failed_storage_is_not_reported_as_absence() {
    let db = Connection::open_in_memory().unwrap();
    assert!(matches!(get_ref(&db, REF), Err(RefReadError::Storage(_))));
}
