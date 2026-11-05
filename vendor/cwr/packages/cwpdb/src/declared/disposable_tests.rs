use super::*;
use crate::{self as cwpdb, CwpdbResult, NativeStore};
use serde_json::json;

const DEFINITION: &str = "cwp://com.example/definitions/event";
const REF: &str = "cwp://com.example/events/one";
fn catalog() -> EntityCatalog {
    crate::desktop::entity_fixture(
        r#"
type = "cwp://org.coworkerprotocol/2026-10-22/schemas/entity-definition"
ref = "cwp://com.example/definitions/event"
storage = "cwpdb"
lifetime = "disposable"
[value_schema]
type = "object"
required = ["body"]
additionalProperties = false
[value_schema.properties.body]
type = "string"
"#,
    )
    .unwrap()
}

#[test]
fn validate_retry_reclaim_and_reject_stale_incarnation() {
    let catalog = catalog();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("graph.sqlite3");
    let mut store = NativeStore::open(&path).unwrap();
    let body = json!({"body":"original"});
    let first = store
        .graph_transaction(|tx| {
            assert!(catalog
                .create_in_transaction(tx, DEFINITION, REF, &json!({"body":false}))
                .is_err());
            let first = catalog.create_in_transaction(tx, DEFINITION, REF, &body)?;
            assert_eq!(
                catalog.insert_disposable_in_transaction(tx, DEFINITION, REF, &body)?,
                InsertOutcome::Existing(first.clone())
            );
            assert!(catalog
                .insert_disposable_in_transaction(tx, DEFINITION, REF, &json!({"body":"changed"}))
                .is_err());
            assert!(catalog
                .create_in_transaction(tx, DEFINITION, REF, &body)
                .is_err());
            Ok(first)
        })
        .unwrap();
    store
        .graph_transaction(|tx| {
            assert_eq!(
                catalog.purge_in_transaction(tx, DEFINITION, REF, &first.revision)?,
                PurgeOutcome::Purged
            );
            assert_eq!(
                catalog.purge_in_transaction(tx, DEFINITION, REF, &first.revision)?,
                PurgeOutcome::Absent
            );
            assert!(cwpdb::find(tx, REF)?.is_none());
            Ok(())
        })
        .unwrap();
    drop(store);
    let mut store = NativeStore::open(&path).unwrap();
    let second = store.create_declared_entity(&catalog, REF, &body).unwrap();
    assert_ne!(first.revision, second.revision);
    store
        .graph_transaction(|tx| {
            assert!(catalog
                .purge_in_transaction(tx, DEFINITION, REF, &first.revision)
                .is_err());
            assert!(catalog
                .replace_in_transaction(tx, DEFINITION, REF, &first.revision, &body)
                .is_err());
            assert!(catalog
                .remove_in_transaction(tx, DEFINITION, REF, &second.revision)
                .is_err());
            assert_eq!(catalog.get_in_connection(tx, DEFINITION, REF)?, second);
            Ok(())
        })
        .unwrap();
}

#[test]
fn purge_protects_durable_edges_and_rolls_back() {
    let catalog = catalog();
    let mut store = NativeStore::open_in_memory().unwrap();
    let first = store
        .create_declared_entity(&catalog, REF, &json!({"body":"x"}))
        .unwrap();
    store
        .graph_transaction(|tx| {
            cwpdb::put_edge(tx, "cwp://com.example/work", "protects", REF, 0, &json!({}))?;
            assert!(catalog
                .purge_in_transaction(tx, DEFINITION, REF, &first.revision)
                .is_err());
            assert_eq!(catalog.get_in_connection(tx, DEFINITION, REF)?, first);
            Ok(())
        })
        .unwrap();
    let mut other = NativeStore::open_in_memory().unwrap();
    let record = other
        .create_declared_entity(&catalog, REF, &json!({"body":"x"}))
        .unwrap();
    let failed: CwpdbResult<()> = other.graph_transaction(|tx| {
        catalog.purge_in_transaction(tx, DEFINITION, REF, &record.revision)?;
        Err("injected failure after purge".into())
    });
    assert!(failed.is_err());
    assert_eq!(other.read_declared_entity(&catalog, REF).unwrap(), record);
}

#[test]
fn repeated_purge_has_constant_identity_count_and_reuses_pages() {
    let catalog = catalog();
    let mut store = NativeStore::open_in_memory().unwrap();
    let mut pages = 0i64;
    for round in 0..200 {
        store
            .graph_transaction(|tx| {
                let record =
                    catalog.create_in_transaction(tx, DEFINITION, REF, &json!({"body":"x"}))?;
                let changed = catalog.replace_in_transaction(
                    tx,
                    DEFINITION,
                    REF,
                    &record.revision,
                    &json!({"body":"y"}),
                )?;
                assert_ne!(record.revision, changed.revision);
                assert!(catalog
                    .purge_in_transaction(tx, DEFINITION, REF, &record.revision)
                    .is_err());
                catalog.purge_in_transaction(tx, DEFINITION, REF, &changed.revision)?;
                assert_eq!(
                    tx.query_row("SELECT count(*) FROM ref_bindings", [], |r| r
                        .get::<_, i64>(0))?,
                    3
                );
                assert_eq!(
                    tx.query_row(
                        "SELECT count(*) FROM nodes WHERE kind='disposable_revision_clock'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    1
                );
                let current = tx.query_row("PRAGMA page_count", [], |r| r.get::<_, i64>(0))?;
                if round == 10 {
                    pages = current;
                }
                if round > 10 {
                    assert_eq!(current, pages);
                }
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn batch_checks_every_revision_before_any_removal() {
    let catalog = catalog();
    let mut store = NativeStore::open_in_memory().unwrap();
    store
        .graph_transaction(|tx| {
            let first = catalog.create_in_transaction(tx, DEFINITION, REF, &json!({"body":"x"}))?;
            let other = "cwp://com.example/events/two";
            let second =
                catalog.create_in_transaction(tx, DEFINITION, other, &json!({"body":"x"}))?;
            let changed = catalog.replace_in_transaction(
                tx,
                DEFINITION,
                other,
                &second.revision,
                &json!({"body":"y"}),
            )?;
            assert!(catalog
                .purge_batch_in_transaction(
                    tx,
                    DEFINITION,
                    &[(REF, &first.revision), (other, &second.revision)]
                )
                .is_err());
            assert_eq!(catalog.get_in_connection(tx, DEFINITION, REF)?, first);
            assert_eq!(catalog.get_in_connection(tx, DEFINITION, other)?, changed);
            assert!(catalog
                .purge_batch_in_transaction(tx, DEFINITION, &vec![(REF, &first.revision); 251])
                .is_err());
            catalog.purge_batch_in_transaction(
                tx,
                DEFINITION,
                &[(REF, &first.revision), (other, &changed.revision)],
            )?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn excessive_relationship_work_and_value_bytes_fail_before_removal() {
    let catalog = catalog();
    let mut store = NativeStore::open_in_memory().unwrap();
    store
        .graph_transaction(|tx| {
            assert!(catalog
                .create_in_transaction(tx, DEFINITION, REF, &json!({"body":"x".repeat(1_048_576)}))
                .is_err());
            assert!(cwpdb::find(tx, REF)?.is_none());
            let first = catalog.create_in_transaction(tx, DEFINITION, REF, &json!({"body":"x"}))?;
            for index in 0..1024 {
                cwpdb::put_disposable_edge(
                    tx,
                    REF,
                    "lookup",
                    &format!("cwp://com.example/index/{index}"),
                    0,
                    &json!({}),
                )?;
            }
            assert!(catalog
                .purge_in_transaction(tx, DEFINITION, REF, &first.revision)
                .unwrap_err()
                .to_string()
                .contains("work limit"));
            assert_eq!(catalog.get_in_connection(tx, DEFINITION, REF)?, first);
            assert_eq!(cwpdb::targets(tx, REF, "lookup")?.len(), 1024);
            Ok(())
        })
        .unwrap();
}

#[test]
fn purged_file_storage_and_checkpointed_wal_stay_flat() {
    let catalog = catalog();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("graph.sqlite3");
    let mut store = NativeStore::open(&path).unwrap();
    let mut baseline = 0;
    for round in 0..40 {
        store
            .graph_transaction(|tx| {
                let reference = format!("cwp://com.example/events/{round}");
                let record = catalog.create_in_transaction(
                    tx,
                    DEFINITION,
                    &reference,
                    &json!({"body":"x".repeat(8192)}),
                )?;
                catalog.purge_in_transaction(tx, DEFINITION, &reference, &record.revision)?;
                Ok(())
            })
            .unwrap();
        assert!(!store.checkpoint_wal().unwrap().busy);
        let size = std::fs::metadata(&path).unwrap().len();
        if round == 5 {
            baseline = size;
        }
        if round > 5 {
            assert_eq!(size, baseline);
        }
        let wal = path.with_extension("sqlite3-wal");
        if let Ok(metadata) = std::fs::metadata(wal) {
            assert_eq!(metadata.len(), 0);
        }
    }
}

#[test]
fn reserved_durable_identity_and_exhausted_revision_clock_fail_closed() {
    let catalog = catalog();
    let mut store = NativeStore::open_in_memory().unwrap();
    store
        .graph_transaction(|tx| {
            cwpdb::bind(tx, REF)?;
            assert!(catalog
                .create_in_transaction(tx, DEFINITION, REF, &json!({"body":"x"}))
                .is_err());
            let other = "cwp://com.example/events/two";
            let record =
                catalog.create_in_transaction(tx, DEFINITION, other, &json!({"body":"x"}))?;
            tx.execute(
                "UPDATE nodes SET props=jsonb('{\"value\":9223372036854775807}') WHERE kind='disposable_revision_clock'",
                [],
            )?;
            assert!(catalog
                .replace_in_transaction(
                    tx,
                    DEFINITION,
                    other,
                    &record.revision,
                    &json!({"body":"y"})
                )
                .is_err());
            assert!(catalog
                .create_in_transaction(
                    tx,
                    DEFINITION,
                    "cwp://com.example/events/three",
                    &json!({"body":"x"})
                )
                .is_err());
            assert_eq!(catalog.get_in_connection(tx, DEFINITION, other)?, record);
            assert!(cwpdb::find(tx, "cwp://com.example/events/three")?.is_none());
            Ok(())
        })
        .unwrap();
}

#[test]
fn purge_refuses_durable_work_that_refers_to_a_disposable_projection() {
    let catalog = catalog();
    let mut store = NativeStore::open_in_memory().unwrap();
    store
        .graph_transaction(|tx| {
            let record =
                catalog.create_in_transaction(tx, DEFINITION, REF, &json!({"body":"x"}))?;
            let target = "cwp://com.example/external";
            cwpdb::put_disposable_edge(tx, REF, "lookup", target, 0, &json!({}))?;
            let projection = cwpdb::derived_ref("lookup", &[REF, target]);
            cwpdb::put_edge(
                tx,
                "cwp://com.example/durable-work",
                "refers-to-projection",
                &projection,
                0,
                &json!({}),
            )?;
            // The event itself remains disposable; protection must inspect the edge
            // subtree, not merely detect promotion of the event's own binding.
            assert!(tx.query_row(
                "SELECT disposable FROM ref_bindings WHERE canonical_ref=?1",
                [REF],
                |r| r.get::<_, bool>(0)
            )?);
            assert!(catalog
                .purge_in_transaction(tx, DEFINITION, REF, &record.revision)
                .is_err());
            assert_eq!(catalog.get_in_connection(tx, DEFINITION, REF)?, record);
            assert_eq!(cwpdb::targets(tx, REF, "lookup")?, [target]);
            assert_eq!(
                cwpdb::targets(tx, "cwp://com.example/durable-work", "refers-to-projection")?,
                [projection]
            );
            Ok(())
        })
        .unwrap();
}
