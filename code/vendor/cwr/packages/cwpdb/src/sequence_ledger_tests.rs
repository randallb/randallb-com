use super::*;
use crate::NativeStore;
use serde_json::json;
const EVENT: &str = "cwp://com.example/types/event";
const STATE: &str = "cwp://com.example/types/state";
const CURSOR: &str = "cwp://com.example/cursor";
fn catalog(reference: &str, disposable: bool) -> EntityCatalog {
    crate::desktop::entity_fixture(&format!(
        r#"
type = "cwp://org.coworkerprotocol/2026-10-22/schemas/entity-definition"
ref = "{reference}"
storage = "cwpdb"
lifetime = "{}"
[value_schema]
type = "object"
"#,
        if disposable { "disposable" } else { "durable" }
    ))
    .unwrap()
}
fn ledger<'a>(events: &'a EntityCatalog, state: &'a EntityCatalog) -> SequenceLedger<'a> {
    SequenceLedger {
        events,
        event_definition: EVENT,
        state,
        state_definition: STATE,
        state_ref: CURSOR,
        state_sequence: "high_water",
        record_envelope: false,
        sequence: "sequence",
        payload: "event",
        bytes: "bytes",
        max_sequence: i64::MAX,
    }
}
fn input(name: &str, body: &str) -> AppendValue {
    AppendValue {
        reference: format!("cwp://com.example/events/{name}"),
        value: json!({"event":body,"received":-1}),
    }
}
#[test]
fn append_attempts_preserve_original_and_deleted_legacy_high_water_across_reopen() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("db");
    let mut store = NativeStore::open(&path).unwrap();
    let events = catalog(EVENT, true);
    let state = catalog(STATE, false);
    store
        .create_declared_entity(&state, CURSOR, &json!({"high_water":20,"owner":"preserve"}))
        .unwrap();
    let ledger = ledger(&events, &state);
    store
        .graph_transaction(|tx| {
            let batch = ledger.append_attempt_batch_in_transaction(
                tx,
                &[input("a", "é"), input("a", "changed"), input("b", "second")],
            )?;
            assert_eq!(batch.high_water, 23);
            assert!(matches!(batch.outcomes[1], InsertOutcome::Existing(_)));
            let original = events.get_in_connection(tx, EVENT, "cwp://com.example/events/a")?;
            assert_eq!(original.value.properties["event"], "é");
            assert_eq!(original.value.properties["bytes"], 2);
            assert_eq!(original.value.properties["sequence"], 21);
            events.purge_in_transaction(tx, EVENT, &original.reference, &original.revision)?;
            Ok(())
        })
        .unwrap();
    drop(store);
    let mut store = NativeStore::open(&path).unwrap();
    store
        .graph_transaction(|tx| {
            let batch =
                ledger.append_attempt_batch_in_transaction(tx, &[input("c", "after reopen")])?;
            assert_eq!(batch.high_water, 24);
            assert_eq!(
                ledger.state_in_transaction(tx)?.value.properties["owner"],
                "preserve"
            );
            Ok(())
        })
        .unwrap();
}
#[test]
fn validation_and_injected_failure_roll_back_cursor_and_events() {
    let root = tempfile::tempdir().unwrap();
    let mut store = NativeStore::open(root.path().join("db")).unwrap();
    let events = catalog(EVENT, true);
    let state = catalog(STATE, false);
    let ledger = ledger(&events, &state);
    store
        .create_declared_entity(&state, CURSOR, &json!({"high_water":0}))
        .unwrap();
    let result: CwpdbResult<()> = store.graph_transaction(|tx| {
        ledger.append_attempt_batch_in_transaction(tx, &[input("a", "valid")])?;
        Err("injected failure after append".into())
    });
    assert!(result.is_err());
    store
        .graph_transaction(|tx| {
            assert_eq!(
                ledger.state_in_transaction(tx)?.value.properties["high_water"],
                0
            );
            assert!(matches!(
                crate::ref_read::get_ref(tx, "cwp://com.example/events/a"),
                Err(RefReadError::NotFound)
            ));
            let mut invalid = input("b", "valid");
            invalid.value["event"] = json!(false);
            assert!(ledger
                .append_attempt_batch_in_transaction(tx, &[input("a", "valid"), invalid])
                .is_err());
            assert!(matches!(
                crate::ref_read::get_ref(tx, "cwp://com.example/events/a"),
                Err(RefReadError::NotFound)
            ));
            Ok(())
        })
        .unwrap();
}
