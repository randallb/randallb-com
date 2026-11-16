//! Sparse expiry traverses byte-bounded cutoff misses in one owning transaction.
use super::*;
use crate::sequence_read::{Direction, ReadBounds, StopReason};
fn retained(path: &Path, first_expired: bool) -> PosthogOutbox {
    let mut db = security::open(path).unwrap();
    db.execute_batch(include_str!("legacy_fixture.sql"))
        .unwrap();
    let tx = db.transaction().unwrap();
    tx.execute("DELETE FROM queue", []).unwrap();
    let original = json!({"padding":"x".repeat(60_000)}).to_string();
    for sequence in 1..=52 {
        let expired = sequence == 52 || (sequence == 1 && first_expired);
        tx.execute("INSERT INTO queue(id,destination,received,event,quarantined) VALUES(?1,'synthetic',?2,?3,?4)",
            rusqlite::params![sequence,if expired {0} else {10_000},original,i64::from(sequence % 3 == 0)]).unwrap();
    }
    tx.commit().unwrap();
    drop(db);
    open(path).unwrap()
}
fn snapshot(db: &Connection) -> Vec<(String, String, i64)> {
    db.prepare("SELECT b.canonical_ref,json(n.props),n.revision FROM nodes n JOIN ref_bindings b ON b.ref_id=n.ref_id ORDER BY b.canonical_ref").unwrap()
        .query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap().collect::<Result<_,_>>().unwrap()
}
#[test]
fn sparse_expiry_traverses_empty_byte_limited_pages_without_skipping_later_match() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut outbox = retained(&path, false);
    let first = reads::admission_index()
        .read(
            &outbox.db,
            &[],
            Direction::Ascending,
            None,
            Some(1),
            ReadBounds {
                rows: 250,
                bytes: 1_048_576,
                work: MAX_QUEUE_EVENTS,
            },
        )
        .unwrap();
    assert!(first.items.is_empty());
    assert!(!first.complete);
    assert_eq!(first.stopped_by, Some(StopReason::Bytes));
    assert!(first.next.is_some_and(|n| n > 0 && n < 52));
    assert!(first.examined > 0);
    assert!(first.bytes <= 1_048_576);
    assert_eq!(outbox.expire(14 * 24 * 60 * 60 * 1000 + 1).unwrap(), 1);
    assert_eq!(outbox.accounting().unwrap().rows, 51);
    assert_eq!(outbox.dropped().unwrap(), 18);
    assert_eq!(outbox.expire(14 * 24 * 60 * 60 * 1000 + 1).unwrap(), 0);
    accounting_tests::assert_totals(&outbox);
    drop(outbox);
    let mut outbox = open(&path).unwrap();
    assert_eq!(outbox.accounting().unwrap().rows, 51);
    assert_eq!(
        outbox.expire(14 * 24 * 60 * 60 * 1000 + 10_001).unwrap(),
        51
    );
    assert_eq!(outbox.dropped().unwrap(), 69);
    accounting_tests::assert_totals(&outbox);
}
#[test]
fn late_expiry_failure_after_empty_pages_rolls_back_earlier_purge_and_totals() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut outbox = retained(&path, true);
    let before = snapshot(&outbox.db);
    outbox.db.execute_batch("CREATE TEMP TRIGGER fail_late_expiry BEFORE DELETE ON nodes WHEN OLD.ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref='cwp://org.coworkerprotocol/cwr/posthog/events/52') BEGIN SELECT RAISE(ABORT,'late expiry failure'); END;").unwrap();
    assert!(outbox.expire(14 * 24 * 60 * 60 * 1000 + 1).is_err());
    assert_eq!(snapshot(&outbox.db), before);
    assert_eq!(outbox.accounting().unwrap().rows, 52);
    assert_eq!(outbox.dropped().unwrap(), 17);
    accounting_tests::assert_totals(&outbox);
    drop(outbox);
    let mut outbox = open(&path).unwrap();
    assert_eq!(snapshot(&outbox.db), before);
    assert_eq!(outbox.expire(14 * 24 * 60 * 60 * 1000 + 1).unwrap(), 2);
    assert_eq!(outbox.accounting().unwrap().rows, 50);
    assert_eq!(outbox.dropped().unwrap(), 19);
    accounting_tests::assert_totals(&outbox);
}
