use crate as cwpdb;
use crate::ref_read::get_ref;
use serde_json::json;

#[test]
fn declared_lifecycle_enforces_binding_revisions_and_rollback() {
    let catalog =
        crate::desktop::entity_fixture(include_str!("../desktop/fixtures/workspace-variable.toml"))
            .unwrap();
    let definition = "cwp://org.coworkerprotocol/wlp/entities/workspace-variable";
    let reference = "cwp://com.example/variables/lifecycle";
    let original = json!({"record":{"name":"ALPHA","kind":"variable","value":"first"}});
    let updated = json!({"record":{"name":"ALPHA","kind":"variable","value":"second"}});
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    {
        let tx = db.transaction().unwrap();
        cwpdb::initialize(&tx).unwrap();
        tx.commit().unwrap();
    }
    let first = {
        let tx = db.transaction().unwrap();
        let result = catalog
            .create_in_transaction(&tx, definition, reference, &original)
            .unwrap();
        tx.commit().unwrap();
        result
    };
    {
        let tx = db.transaction().unwrap();
        catalog
            .replace_in_transaction(&tx, definition, reference, &first.revision, &updated)
            .unwrap();
        // Do not commit: the domain may fail after changing the record.
    }
    assert_eq!(
        catalog
            .get_in_connection(&db, definition, reference)
            .unwrap(),
        first
    );
    let second = {
        let tx = db.transaction().unwrap();
        let result = catalog
            .replace_in_transaction(&tx, definition, reference, &first.revision, &updated)
            .unwrap();
        tx.commit().unwrap();
        result
    };
    assert_ne!(first.revision, second.revision);
    assert_eq!(second.value.properties, updated);
    {
        let tx = db.transaction().unwrap();
        assert!(catalog
            .replace_in_transaction(&tx, definition, reference, &first.revision, &original)
            .is_err());
        assert!(catalog
            .remove_in_transaction(&tx, definition, reference, &first.revision)
            .is_err());
        assert!(catalog
            .replace_in_transaction(
                &tx,
                definition,
                reference,
                &second.revision,
                &json!({"record":{"name":"bad name","kind":"variable","value":"x"}})
            )
            .is_err());
        assert!(catalog
            .replace_in_transaction(
                &tx,
                definition,
                "cwp://com.example/missing",
                &second.revision,
                &updated
            )
            .is_err());
        tx.commit().unwrap();
    }
    assert_eq!(
        catalog
            .get_in_connection(&db, definition, reference)
            .unwrap(),
        second
    );
    {
        let tx = db.transaction().unwrap();
        catalog
            .remove_in_transaction(&tx, definition, reference, &second.revision)
            .unwrap();
    }
    assert_eq!(
        catalog
            .get_in_connection(&db, definition, reference)
            .unwrap(),
        second
    );
    {
        let tx = db.transaction().unwrap();
        catalog
            .remove_in_transaction(&tx, definition, reference, &second.revision)
            .unwrap();
        tx.commit().unwrap();
    }
    assert!(get_ref(&db, reference).is_err());
    {
        let tx = db.transaction().unwrap();
        assert!(catalog
            .create_in_transaction(&tx, definition, reference, &original)
            .is_err());
    }
    let other = "cwp://com.example/other";
    {
        let tx = db.transaction().unwrap();
        cwpdb::put_node_with_schema(&tx, other, "other", "cwp://com.example/schema", &original)
            .unwrap();
        tx.commit().unwrap();
    }
    let wrong = get_ref(&db, other).unwrap();
    {
        let tx = db.transaction().unwrap();
        assert!(catalog.get_in_connection(&tx, definition, other).is_err());
        assert!(catalog
            .replace_in_transaction(&tx, definition, other, &wrong.revision, &updated)
            .is_err());
        assert!(catalog
            .remove_in_transaction(&tx, definition, other, &wrong.revision)
            .is_err());
        tx.commit().unwrap();
    }
    assert_eq!(get_ref(&db, other).unwrap(), wrong);
}

#[test]
fn replacement_removes_omitted_fields_and_keeps_explicit_nulls() {
    let catalog = crate::desktop::entity_fixture(
        r#"
type = "cwp://org.coworkerprotocol/2026-10-22/schemas/entity-definition"
ref = "cwp://com.example/entities/object"
storage = "cwpdb"
[value_schema]
type = "object"
"#,
    )
    .unwrap();
    let definition = "cwp://com.example/entities/object";
    let reference = "cwp://com.example/object/one";
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    {
        let tx = db.transaction().unwrap();
        cwpdb::initialize(&tx).unwrap();
        tx.commit().unwrap();
    }
    let tx = db.transaction().unwrap();
    let first = catalog
        .create_in_transaction(
            &tx,
            definition,
            reference,
            &json!({"obsolete":true,"nested":{"old":1},"nullable":"before"}),
        )
        .unwrap();
    let replacement = json!({"nested":{"new":2},"nullable":null});
    let second = catalog
        .replace_in_transaction(&tx, definition, reference, &first.revision, &replacement)
        .unwrap();
    assert_eq!(second.value.properties, replacement);
    assert_eq!(
        second.revision.legacy_counter(),
        first.revision.legacy_counter() + 1
    );
    tx.commit().unwrap();
    assert_eq!(
        catalog
            .get_in_connection(&db, definition, reference)
            .unwrap(),
        second
    );
}
