use crate::{bind, put_edge, put_node, remove, NativeStore};
use serde_json::json;
const REF: &str = "cwp://com.example/entities/one";
fn catalog() -> crate::declared::EntityBinding {
    crate::desktop::entity_fixture(include_str!("../desktop/fixtures/variable.toml")).unwrap()
}
#[test]
fn creation_rejects_retirement_and_edges_but_allows_unused_binding() {
    let catalog = catalog();
    let mut store = NativeStore::open_in_memory().unwrap();
    let edge = store
        .graph_transaction(|tx| {
            bind(tx, REF)?;
            put_node(tx, "cwp://com.example/deleted", "old", &json!({}))?;
            remove(tx, "cwp://com.example/deleted")?;
            put_edge(
                tx,
                "cwp://com.example/source",
                "test",
                "cwp://com.example/target",
                0,
                &json!({}),
            )?;
            Ok(crate::derived_ref(
                "test",
                &["cwp://com.example/source", "cwp://com.example/target"],
            ))
        })
        .unwrap();
    let value = json!({"name":"TEST","value":"x"});
    store.create_declared_entity(&catalog, REF, &value).unwrap();
    assert!(store
        .create_declared_entity(&catalog, "cwp://com.example/deleted", &value)
        .unwrap_err()
        .to_string()
        .contains("retired"));
    assert!(store
        .create_declared_entity(&catalog, &edge, &value)
        .unwrap_err()
        .to_string()
        .contains("wrong_record_type"));
}
#[test]
fn domain_merge_validates_complete_value_and_preserves_graph_patch_semantics() {
    let catalog = crate::desktop::entity_fixture(
        r#"
type = "cwp://org.coworkerprotocol/2026-10-22/schemas/entity-definition"
ref = "cwp://com.example/definitions/record"
storage = "cwpdb"
record_kind = "example"
record_schema = "cwp://org.coworkerprotocol/cwpdb/schemas/example/2026-09"
[value_schema]
type = "object"
required = ["record"]
[value_schema.properties.record]
type = "object"
required = ["text"]
[value_schema.properties.record.properties.text]
type = "string"
"#,
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut store = NativeStore::open(directory.path().join("merge.sqlite")).unwrap();
    let definition = "cwp://com.example/definitions/record";
    store
        .graph_transaction(|tx| {
            catalog.merge_in_transaction(
                tx,
                definition,
                REF,
                &json!({"record":{"text":"first","old":true},"metadata":1}),
            )?;
            catalog.merge_in_transaction(
                tx,
                definition,
                REF,
                &json!({"record":{"text":"second","nullable":null}}),
            )
        })
        .unwrap();
    let before = store.read_declared_entity(&catalog, REF).unwrap();
    assert_eq!(
        before.value.properties,
        json!({"record":{"text":"second"},"metadata":1})
    );
    // Catching validation failure inside a caller transaction cannot leave a write.
    store
        .graph_transaction(|tx| {
            assert!(catalog
                .merge_in_transaction(
                    tx,
                    definition,
                    REF,
                    &json!({"record":{"text":false},"metadata":2})
                )
                .is_err());
            Ok(())
        })
        .unwrap();
    assert_eq!(store.read_declared_entity(&catalog, REF).unwrap(), before);
}
