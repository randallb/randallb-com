use super::*;
use crate::{initialize, put_disposable_node};
use serde_json::json;
fn index() -> SequenceIndex {
    SequenceIndex {
        name: "test_sequence".into(),
        kind: "outbox".into(),
        schema: crate::schema_ref("outbox"),
        sequence: "sequence".into(),
        receipt: "received".into(),
        record_envelope: true,
        equalities: vec!["destination".into(), "quarantine".into()],
    }
}
fn seed(tx: &Transaction<'_>, i: i64, destination: &str) {
    let value = json!({"record":{"sequence":i,"received":i,"destination":destination,"quarantine":0,"event":"body"}});
    let reference = format!("cwp://example/events/{i}");
    put_disposable_node(tx, &reference, "outbox", &value).unwrap();
}

fn limits() -> ReadBounds {
    ReadBounds {
        rows: 2,
        bytes: 10000,
        work: 10,
    }
}
fn filters() -> [SqlValue; 2] {
    [SqlValue::Text("a".into()), SqlValue::Integer(0)]
}
#[test]
fn keysets_join_transaction_and_skip_other_groups() {
    let mut db = Connection::open_in_memory().unwrap();
    let tx = db.transaction().unwrap();
    initialize(&tx).unwrap();
    index().install(&tx).unwrap();
    for i in 1..=5 {
        seed(&tx, i, "a");
    }
    for i in 6..=1005 {
        seed(&tx, i, "unrelated");
    }
    let p = index()
        .read(&tx, &filters(), Direction::Ascending, None, None, limits())
        .unwrap();
    assert_eq!(
        p.items.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(p.next, Some(2));
    assert!(!p.complete);
    assert_eq!(p.examined, 2);
    tx.commit().unwrap();
    let tx = db.transaction().unwrap();
    crate::remove_disposable(&tx, "cwp://example/events/3").unwrap();
    seed(&tx, 1006, "a");
    let p = index()
        .read(
            &tx,
            &filters(),
            Direction::Ascending,
            Some(2),
            Some(6),
            limits(),
        )
        .unwrap();
    assert_eq!(
        p.items.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        vec![4, 5]
    );
    assert!(p.complete);
    assert_eq!(p.next, None);
    tx.rollback().unwrap();
    let p = index()
        .read(&db, &filters(), Direction::Descending, None, None, limits())
        .unwrap();
    assert_eq!(
        p.items.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        vec![5, 4]
    );
    assert!(!p.complete);
}
#[test]
fn sparse_cutoff_work_and_byte_limits_never_claim_completion() {
    let mut db = Connection::open_in_memory().unwrap();
    let tx = db.transaction().unwrap();
    initialize(&tx).unwrap();
    index().install(&tx).unwrap();
    for i in 1..=5 {
        seed(&tx, i, "a");
    }
    tx.commit().unwrap();
    let p = index()
        .read(
            &db,
            &filters(),
            Direction::Ascending,
            None,
            Some(0),
            ReadBounds {
                work: 2,
                ..limits()
            },
        )
        .unwrap();
    assert!(p.items.is_empty());
    assert!(!p.complete);
    assert_eq!(p.next, Some(2));
    assert_eq!(p.examined, 2);
    let p = index()
        .read(
            &db,
            &filters(),
            Direction::Ascending,
            None,
            None,
            ReadBounds {
                bytes: 1,
                ..limits()
            },
        )
        .unwrap();
    assert!(p.items.is_empty());
    assert!(!p.complete);
    assert_eq!(p.next, None);
    assert!(index()
        .read(&db, &[], Direction::Ascending, None, None, limits())
        .is_err());
    let p = index()
        .read(
            &db,
            &filters(),
            Direction::Ascending,
            Some(5),
            None,
            limits(),
        )
        .unwrap();
    assert!(p.complete);
    assert!(p.items.is_empty());
}

#[test]
fn index_work_does_not_scan_unrelated_destinations_and_migration_rejects_conflicts() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let mut db = Connection::open_in_memory().unwrap();
    let tx = db.transaction().unwrap();
    initialize(&tx).unwrap();
    index().install(&tx).unwrap();
    index().install(&tx).unwrap();
    let mut conflict = index();
    conflict.sequence = "other".into();
    assert!(conflict.install(&tx).is_err());
    for i in 1..=5 {
        seed(&tx, i, "a");
    }
    tx.commit().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let observed = counter.clone();
    db.progress_handler(
        1,
        Some(move || {
            observed.fetch_add(1, Ordering::Relaxed);
            false
        }),
    )
    .unwrap();
    index()
        .read(&db, &filters(), Direction::Ascending, None, None, limits())
        .unwrap();
    let small = counter.swap(0, Ordering::Relaxed);
    let tx = db.transaction().unwrap();
    for i in 6..=5005 {
        seed(&tx, i, "unrelated");
    }
    tx.commit().unwrap();
    counter.store(0, Ordering::Relaxed);
    index()
        .read(&db, &filters(), Direction::Ascending, None, None, limits())
        .unwrap();
    let large = counter.load(Ordering::Relaxed);
    assert!(
        large <= small + 100,
        "SQLite steps grew from {small} to {large}"
    );
    eprintln!("sequence read SQLite VM steps: {small} before, {large} after 5000 unrelated rows");
}

#[test]
fn flat_accounting_includes_quarantine_and_original_utf8_bytes() {
    let mut db = Connection::open_in_memory().unwrap();
    let tx = db.transaction().unwrap();
    initialize(&tx).unwrap();
    let mut spec = index();
    spec.record_envelope = false;
    spec.equalities = vec!["destination".into(), "quarantined".into()];
    spec.install(&tx).unwrap();
    for (sequence, quarantined, event) in [(1, false, "é"), (2, true, "broken json")] {
        let value = json!({"sequence":sequence,"received":-100,"destination":"a","quarantined":quarantined,"event":event,"bytes":event.len()});
        crate::put_disposable_node(
            &tx,
            &format!("cwp://example/flat/{sequence}"),
            "outbox",
            &value,
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let count = spec.account(&db, "bytes", 10).unwrap();
    assert_eq!(
        count,
        SequenceAccounting {
            rows: 2,
            bytes: 13,
            max_live_sequence: 2,
            complete: true
        }
    );
    let count = spec.account(&db, "bytes", 1).unwrap();
    assert!(!count.complete);
    assert_eq!(count.rows, 1);
    let p = spec
        .read(
            &db,
            &[SqlValue::Text("a".into()), SqlValue::Integer(1)],
            Direction::Ascending,
            None,
            Some(-99),
            limits(),
        )
        .unwrap();
    assert_eq!(p.items.len(), 1);
    assert_eq!(p.items[0].sequence, 2);
    assert!(p.complete);
}

#[test]
fn maximum_i64_sequence_is_not_lost_on_initial_descending_page() {
    let mut db = Connection::open_in_memory().unwrap();
    let tx = db.transaction().unwrap();
    initialize(&tx).unwrap();
    index().install(&tx).unwrap();
    seed(&tx, i64::MAX, "a");
    let page = index()
        .read(&tx, &filters(), Direction::Descending, None, None, limits())
        .unwrap();
    assert_eq!(page.items[0].sequence, i64::MAX);
    assert!(page.complete);
}

#[test]
fn index_build_full_rolls_back_and_reopens_retained_records_without_cursor_loss() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("graph.sqlite3");
    let mut db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA recursive_triggers=ON;")
        .unwrap();
    let tx = db.transaction().unwrap();
    initialize(&tx).unwrap();
    for sequence in 1..=1000 {
        seed(&tx, sequence, "retained");
    }
    crate::put_node(
        &tx,
        "cwp://com.example/high_water",
        "cursor",
        &json!({"sequence":2000}),
    )
    .unwrap();
    tx.commit().unwrap();
    let pages: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    db.pragma_update(None, "max_page_count", pages).unwrap();
    let tx = db.transaction().unwrap();
    let error = index().install(&tx).unwrap_err();
    assert!(error.to_string().contains("full"), "{error}");
    drop(tx);
    assert_eq!(
        crate::node_props(&db, "cwp://com.example/high_water")
            .unwrap()
            .unwrap()["sequence"],
        2000
    );
    let present: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='test_sequence')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!present);
    db.pragma_update(None, "max_page_count", pages + 1000)
        .unwrap();
    let tx = db.transaction().unwrap();
    index().install(&tx).unwrap();
    tx.commit().unwrap();
    drop(db);
    let db = Connection::open(&path).unwrap();
    let page = index()
        .read(
            &db,
            &[SqlValue::Text("retained".into()), SqlValue::Integer(0)],
            Direction::Ascending,
            None,
            None,
            limits(),
        )
        .unwrap();
    assert_eq!(page.items[0].sequence, 1);
    assert!(!page.complete);
}

#[test]
fn byte_work_budget_counts_cutoff_misses_and_returns_a_progressing_empty_page() {
    let mut db = Connection::open_in_memory().unwrap();
    let tx = db.transaction().unwrap();
    initialize(&tx).unwrap();
    index().install(&tx).unwrap();
    for i in 1..=5 {
        seed(&tx, i, "a");
    }
    let one = crate::node_props(&tx, "cwp://example/events/1")
        .unwrap()
        .unwrap()
        .to_string()
        .len();
    let page = index()
        .read(
            &tx,
            &filters(),
            Direction::Ascending,
            None,
            Some(0),
            ReadBounds {
                bytes: one,
                ..limits()
            },
        )
        .unwrap();
    assert!(page.items.is_empty());
    assert!(!page.complete);
    assert_eq!(page.next, Some(1));
    assert_eq!(page.examined, 1);
    assert_eq!(page.bytes, one);
    assert_eq!(page.stopped_by, Some(StopReason::Bytes));
}
