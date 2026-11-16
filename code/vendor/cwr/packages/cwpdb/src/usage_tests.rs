use super::*;
use crate::NativeStore;
fn event(uuid: &str, time: i64) -> Value {
    json!({"uuid":uuid,"event":"navigation","sdkTime":"2026-10-22T00:00:00Z","json":"{\"body\":\"é\"}","control":"nav","receiptMs":time,"hostSession":"host","renderer":"window","appVersion":"test"})
}
#[test]
fn usage_retains_uuids_filters_receipts_and_cursor_after_clear_reopen() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("db");
    let mut store = NativeStore::open(&path).unwrap();
    let ledger = UsageLedger::new(&crate::desktop::test_catalog().unwrap()).unwrap();
    let first = "00000000-0000-0000-0000-000000000001";
    let second = "00000000-0000-0000-0000-000000000002";
    store
        .graph_transaction(|tx| {
            ledger.initialize_in_transaction(tx)?;
            assert_eq!(
                ledger
                    .append_in_transaction(
                        tx,
                        &[event(first, -100), event(first, 900), event(second, 10)]
                    )?
                    .high_water,
                3
            );
            for mask in 0..8 {
                let filters = [
                    if mask & 1 != 0 {
                        Some("navigation")
                    } else {
                        None
                    },
                    if mask & 2 != 0 { Some("nav") } else { None },
                    if mask & 4 != 0 { Some("host") } else { None },
                ];
                let page = ledger.page_in_transaction(tx, filters, None)?;
                assert_eq!(page.items.len(), 2);
                assert_eq!(page.items[1].value["record"]["receiptMs"], -100);
            }
            let expired = ledger.expire_batch_in_transaction(tx, 0, None)?;
            assert_eq!(expired.removed, 1);
            let cleared = ledger.clear_batch_in_transaction(tx)?;
            assert_eq!(cleared.removed, 1);
            assert!(cleared.complete);
            assert!(ledger
                .page_in_transaction(tx, [None, None, None], None)?
                .items
                .is_empty());
            Ok(())
        })
        .unwrap();
    drop(store);
    let mut store = NativeStore::open(&path).unwrap();
    store
        .graph_transaction(|tx| {
            ledger.initialize_in_transaction(tx)?;
            assert_eq!(
                ledger
                    .append_in_transaction(tx, &[event(second, 20)])?
                    .high_water,
                4
            );
            assert_eq!(
                ledger
                    .page_in_transaction(tx, [None, None, None], None)?
                    .items[0]
                    .sequence,
                4
            );
            Ok(())
        })
        .unwrap();
}
#[test]
fn usage_refuses_obsolete_samples_and_missing_high_water() {
    let root = tempfile::tempdir().unwrap();
    let mut store = NativeStore::open(root.path().join("db")).unwrap();
    let ledger = UsageLedger::new(&crate::desktop::test_catalog().unwrap()).unwrap();
    store.graph_transaction(|tx| {tx.execute_batch("CREATE TABLE usage_events(event TEXT); INSERT INTO usage_events VALUES('preserve')")?;
        assert!(ledger.initialize_in_transaction(tx).is_err());let sample:String=tx.query_row("SELECT event FROM usage_events",[],|r|r.get(0))?;assert_eq!(sample,"preserve");Ok(())}).unwrap();
}

#[test]
fn public_projection_keeps_fifty_large_samples_and_resumable_clear_blocks_capture() {
    let root = tempfile::tempdir().unwrap();
    let mut store = NativeStore::open(root.path().join("db")).unwrap();
    let ledger = UsageLedger::new(&crate::desktop::test_catalog().unwrap()).unwrap();
    store
        .graph_transaction(|tx| {
            ledger.initialize_in_transaction(tx)?;
            for batch in 0..15 {
                let rows = (0..7)
                    .map(|offset| {
                        let mut row = event(
                            &format!("00000000-0000-0000-0000-{:012}", batch * 7 + offset + 1),
                            current_time(),
                        );
                        row["json"] = json!("x".repeat(64000));
                        row
                    })
                    .collect::<Vec<_>>();
                ledger.append_in_transaction(tx, &rows)?;
            }
            let page = ledger.inspect_page_in_transaction(tx, [None, None, None], None)?;
            assert_eq!(page["rows"].as_array().unwrap().len(), 50);
            assert_eq!(page["next"], 56);
            assert!(page["rows"][0].get("json").is_none());
            let second = ledger.inspect_page_in_transaction(tx, [None, None, None], Some(56))?;
            assert_eq!(second["rows"].as_array().unwrap().len(), 50);
            let progress = ledger.clear_batch_in_transaction(tx)?;
            assert!(!progress.complete);
            assert!(ledger.clearing_in_transaction(tx)?);
            assert!(ledger
                .append_in_transaction(tx, &[event("00000000-0000-0000-0000-999999999999", 20)])
                .is_err());
            Ok(())
        })
        .unwrap();
    store
        .graph_transaction(|tx| {
            while !ledger.clear_batch_in_transaction(tx)?.complete {}
            assert!(!ledger.clearing_in_transaction(tx)?);
            assert_eq!(
                ledger
                    .append_in_transaction(
                        tx,
                        &[event("00000000-0000-0000-0000-999999999999", 20)]
                    )?
                    .high_water,
                106
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn request_replay_does_not_allocate_again_and_erasure_expires_receipt_without_reexecution() {
    let root = tempfile::tempdir().unwrap();
    let mut store = NativeStore::open(root.path().join("db")).unwrap();
    let ledger = UsageLedger::new(&crate::desktop::test_catalog().unwrap()).unwrap();
    let sample = event("00000000-0000-0000-0000-000000000001", current_time());
    store
        .graph_transaction(|tx| {
            ledger.initialize_in_transaction(tx)?;
            assert_eq!(
                ledger.append_once_in_transaction(
                    tx,
                    "request_a",
                    std::slice::from_ref(&sample)
                )?,
                AppendOnce::Committed { duplicates: 0 }
            );
            assert_eq!(
                ledger.append_once_in_transaction(
                    tx,
                    "request_a",
                    std::slice::from_ref(&sample)
                )?,
                AppendOnce::Committed { duplicates: 0 }
            );
            let state = ledger.state.get_in_connection(tx, CURSOR, CURSOR_REF)?;
            assert_eq!(state.value.properties["record"]["sequence"], 1);
            assert_eq!(ledger.account_in_transaction(tx)?.rows, 1);
            let mut changed = sample.clone();
            changed["event"] = json!("changed");
            assert!(ledger
                .append_once_in_transaction(tx, "request_a", &[changed])
                .is_err());
            assert!(ledger.clear_batch_in_transaction(tx)?.complete);
            assert_eq!(
                ledger.account_in_transaction(tx)?,
                crate::sequence_accounting_ledger::Accounting { rows: 0, bytes: 0 }
            );
            assert_eq!(
                ledger.append_once_in_transaction(
                    tx,
                    "request_a",
                    std::slice::from_ref(&sample)
                )?,
                AppendOnce::CommittedResultExpired
            );
            assert!(ledger
                .page_in_transaction(tx, [None, None, None], None)?
                .items
                .is_empty());
            Ok(())
        })
        .unwrap();
}

#[test]
fn purging_any_batch_dependency_erases_the_request_comparison_and_result() {
    let root = tempfile::tempdir().unwrap();
    let mut store = NativeStore::open(root.path().join("db")).unwrap();
    let ledger = UsageLedger::new(&crate::desktop::test_catalog().unwrap()).unwrap();
    let rows = [
        event(
            "00000000-0000-0000-0000-000000000001",
            current_time() - 15 * 24 * 60 * 60 * 1000,
        ),
        event("00000000-0000-0000-0000-000000000002", current_time()),
    ];
    store
        .graph_transaction(|tx| {
            ledger.initialize_in_transaction(tx)?;
            ledger.append_once_in_transaction(tx, "two_records", &rows)?;
            ledger.expire_batch_in_transaction(
                tx,
                current_time() - 14 * 24 * 60 * 60 * 1000,
                None,
            )?;
            assert_eq!(
                ledger.append_once_in_transaction(tx, "two_records", &[rows[1].clone()])?,
                AppendOnce::CommittedResultExpired
            );
            assert_eq!(
                ledger
                    .page_in_transaction(tx, [None, None, None], None)?
                    .items
                    .len(),
                1
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn retained_validation_rejects_cursor_loss_and_malformed_sequence_without_changing_payload() {
    let root = tempfile::tempdir().unwrap();
    let mut store = NativeStore::open(root.path().join("db")).unwrap();
    let ledger = UsageLedger::new(&crate::desktop::test_catalog().unwrap()).unwrap();
    store.graph_transaction(|tx| {
        let mut value=event("00000000-0000-0000-0000-000000000001",0);value["sequence"]=json!(21);value["sdkVersion"]=json!("1.288.0");value["bytes"]=json!(value["json"].as_str().unwrap().len());
        cwpdb::put_disposable_node(tx,"cwp://getworkloop.com/desktop/local-usage/events/00000000-0000-0000-0000-000000000001","desktop_usage_event",&json!({"record":value}))?;
        assert!(ledger.initialize_in_transaction(tx).is_err());
        ledger.state.create_in_transaction(tx,CURSOR,CURSOR_REF,&json!({"record":{"sequence":20}}))?;
        assert!(ledger.initialize_in_transaction(tx).is_err());
        assert_eq!(cwpdb::node_props(tx,"cwp://getworkloop.com/desktop/local-usage/events/00000000-0000-0000-0000-000000000001")?.unwrap()["record"]["sequence"],21);
        Ok(())
    }).unwrap();
}

fn current_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[test]
fn database_full_retries_same_request_without_partial_events_accounting_or_sequence() {
    let root = tempfile::tempdir().unwrap();
    let mut store = NativeStore::open(root.path().join("db")).unwrap();
    let ledger = UsageLedger::new(&crate::desktop::test_catalog().unwrap()).unwrap();
    let bytes = store
        .graph_transaction(|tx| {
            ledger.initialize_in_transaction(tx)?;
            let pages: i64 = tx.query_row("PRAGMA page_count", [], |r| r.get(0))?;
            let size: i64 = tx.query_row("PRAGMA page_size", [], |r| r.get(0))?;
            Ok((pages * size) as u64)
        })
        .unwrap();
    store.set_max_database_bytes(bytes).unwrap();
    let mut sample = event("00000000-0000-0000-0000-000000000001", current_time());
    sample["json"] = json!("x".repeat(64000));
    let error = store
        .graph_transaction(|tx| {
            ledger.append_once_in_transaction(tx, "retry_full", std::slice::from_ref(&sample))
        })
        .unwrap_err();
    assert!(error.to_string().contains("full"), "{error}");
    store
        .graph_transaction(|tx| {
            assert_eq!(ledger.account_in_transaction(tx)?.rows, 0);
            assert_eq!(
                ledger
                    .state
                    .get_in_connection(tx, CURSOR, CURSOR_REF)?
                    .value
                    .properties["record"]["sequence"],
                0
            );
            Ok(())
        })
        .unwrap();
    store.set_max_database_bytes(bytes + 1024 * 1024).unwrap();
    store
        .graph_transaction(|tx| {
            assert_eq!(
                ledger.append_once_in_transaction(
                    tx,
                    "retry_full",
                    std::slice::from_ref(&sample)
                )?,
                AppendOnce::Committed { duplicates: 0 }
            );
            assert_eq!(ledger.account_in_transaction(tx)?.rows, 1);
            Ok(())
        })
        .unwrap();
}

#[test]
fn elapsed_retention_erases_receipt_before_replay_even_without_explicit_clear() {
    let root = tempfile::tempdir().unwrap();
    let mut store = NativeStore::open(root.path().join("db")).unwrap();
    let ledger = UsageLedger::new(&crate::desktop::test_catalog().unwrap()).unwrap();
    let old = event(
        "00000000-0000-0000-0000-000000000001",
        current_time() - 15 * 24 * 60 * 60 * 1000,
    );
    store
        .graph_transaction(|tx| {
            ledger.initialize_in_transaction(tx)?;
            ledger.append_once_in_transaction(tx, "old_request", std::slice::from_ref(&old))?;
            assert_eq!(
                ledger.append_once_in_transaction(tx, "old_request", std::slice::from_ref(&old))?,
                AppendOnce::CommittedResultExpired
            );
            assert_eq!(ledger.account_in_transaction(tx)?.rows, 0);
            Ok(())
        })
        .unwrap();
}

#[test]
fn startup_rejects_hidden_zero_and_duplicate_sequences_without_publishing_totals() {
    for duplicate in [false, true] {
        let mut store = NativeStore::open_in_memory().unwrap();
        let ledger = UsageLedger::new(&crate::desktop::test_catalog().unwrap()).unwrap();
        store
            .graph_transaction(|tx| {
                ledger.state.create_in_transaction(
                    tx,
                    CURSOR,
                    CURSOR_REF,
                    &json!({"record":{"sequence":251}}),
                )?;
                for number in 1..=if duplicate { 251 } else { 1 } {
                    let uuid = format!("00000000-0000-0000-0000-{number:012}");
                    let mut value = event(&uuid, 0);
                    value["sequence"] = json!(if duplicate { 1 } else { 0 });
                    value["sdkVersion"] = json!("1.288.0");
                    value["bytes"] = json!(value["json"].as_str().unwrap().len());
                    cwpdb::put_disposable_node(
                        tx,
                        &format!("cwp://getworkloop.com/desktop/local-usage/events/{uuid}"),
                        "desktop_usage_event",
                        &json!({"record":value}),
                    )?;
                }
                Ok(())
            })
            .unwrap();
        assert!(store
            .graph_transaction(|tx| ledger.initialize_in_transaction(tx))
            .is_err());
        store
            .graph_transaction(|tx| {
                let cursor = ledger.state.get_in_connection(tx, CURSOR, CURSOR_REF)?;
                assert!(cursor.value.properties["record"].get("rows").is_none());
                assert!(cursor.value.properties["record"].get("bytes").is_none());
                let count: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM nodes WHERE kind='desktop_usage_event'",
                    [],
                    |r| r.get(0),
                )?;
                assert_eq!(count, if duplicate { 251 } else { 1 });
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn startup_validation_refuses_budget_exhaustion_instead_of_accepting_prefix_totals() {
    let mut store = NativeStore::open_in_memory().unwrap();
    let ledger = UsageLedger::new(&crate::desktop::test_catalog().unwrap()).unwrap();
    store
        .graph_transaction(|tx| {
            ledger.initialize_in_transaction(tx)?;
            ledger.append_in_transaction(
                tx,
                &[
                    event("00000000-0000-0000-0000-000000000001", 0),
                    event("00000000-0000-0000-0000-000000000002", 0),
                ],
            )?;
            Ok(())
        })
        .unwrap();
    store
        .graph_transaction(|tx| {
            let before = ledger.account_in_transaction(tx)?;
            assert!(ledger
                .validate_retained_with_limits(tx, 1, 128 * 1024 * 1024)
                .is_err());
            assert!(ledger.validate_retained_with_limits(tx, 2, 1).is_err());
            assert_eq!(ledger.account_in_transaction(tx)?, before);
            assert_eq!(
                ledger.validate_retained_with_limits(tx, 2, 128 * 1024 * 1024)?,
                (before.rows, before.bytes)
            );
            Ok(())
        })
        .unwrap();
}
