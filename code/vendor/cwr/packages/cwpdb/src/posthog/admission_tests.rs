use super::*;
fn event(padding: usize) -> Value {
    json!({"uuid":uuid::Uuid::new_v4().to_string(),"event":"cwr_metric","timestamp":"2026-10-23T00:00:00Z","properties":{"padding":"x".repeat(padding)}})
}
fn stored(outbox: &PosthogOutbox, id: i64) -> crate::ref_read::RefRecord {
    outbox
        .declarations
        .event
        .get_in_connection(&outbox.db, EVENT_DEFINITION, &event_ref(id))
        .unwrap()
}
#[test]
fn malformed_json_is_quarantined_and_destinations_remain_isolated_after_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    let db = security::open(&path).unwrap();
    db.execute_batch(include_str!("legacy_fixture.sql"))
        .unwrap();
    drop(db);
    let mut outbox = open(&path).unwrap();
    let raw = stored(&outbox, 1).value.properties;
    let rows = outbox.pending("synthetic-destination-a", 100).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(outbox
        .pending("synthetic-destination-b", 100)
        .unwrap()
        .is_empty());
    assert_eq!(
        stored(&outbox, 9).value.properties["record"]["quarantined"],
        true
    );
    assert_eq!(outbox.accounting().unwrap().rows, 3);
    assert_eq!(outbox.dropped().unwrap(), 17);
    drop(outbox);
    let mut outbox = open(&path).unwrap();
    assert_eq!(
        outbox.pending("synthetic-destination-a", 100).unwrap(),
        rows
    );
    assert_eq!(stored(&outbox, 1).value.properties, raw);
    assert_eq!(outbox.expire(14 * 24 * 60 * 60 * 1000 + 3001).unwrap(), 3);
    assert_eq!(outbox.dropped().unwrap(), 20);
    assert_eq!(outbox.accounting().unwrap().rows, 0);
}
#[test]
fn expiry_counter_failure_rolls_back_records_and_quarantine_is_counted() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    let mut outbox = open(&path).unwrap();
    outbox
        .enqueue("synthetic", 0, &[event(0), event(0)])
        .unwrap();
    let selection = outbox.pending("synthetic", 100).unwrap()[0].0.clone();
    outbox.quarantine(&[selection]).unwrap();
    outbox.db.execute_batch("CREATE TEMP TRIGGER fail_drop BEFORE UPDATE ON nodes WHEN OLD.kind='posthog_state' AND json_extract(NEW.props,'$.record.counters.dropped')>json_extract(OLD.props,'$.record.counters.dropped') BEGIN SELECT RAISE(ABORT,'injected drop counter failure'); END;").unwrap();
    assert!(outbox.expire(14 * 24 * 60 * 60 * 1000 + 1).is_err());
    assert_eq!(outbox.accounting().unwrap().rows, 2);
    assert_eq!(outbox.dropped().unwrap(), 0);
    drop(outbox);
    let mut outbox = open(&path).unwrap();
    assert_eq!(outbox.expire(14 * 24 * 60 * 60 * 1000 + 1).unwrap(), 2);
    assert_eq!(outbox.dropped().unwrap(), 2);
    outbox
        .enqueue("synthetic", 14 * 24 * 60 * 60 * 1000 + 2, &[event(0)])
        .unwrap();
    assert_eq!(stored(&outbox, 3).value.properties["record"]["sequence"], 3);
}
#[test]
fn byte_capacity_includes_quarantine_and_eviction_counter_failure_is_atomic() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    let mut outbox = open(&path).unwrap();
    outbox
        .enqueue(
            "synthetic",
            1000,
            &(0..139).map(|_| event(60_000)).collect::<Vec<_>>(),
        )
        .unwrap();
    assert_eq!(outbox.dropped().unwrap(), 0);
    let oldest = outbox.pending("synthetic", 100).unwrap()[0].0.clone();
    outbox.quarantine(&[oldest]).unwrap();
    let original = stored(&outbox, 1);
    outbox.db.execute_batch("CREATE TEMP TRIGGER fail_evict BEFORE UPDATE ON nodes WHEN OLD.kind='posthog_state' AND json_extract(NEW.props,'$.record.counters.dropped')>0 BEGIN SELECT RAISE(ABORT,'injected eviction counter failure'); END;").unwrap();
    assert!(outbox.enqueue("synthetic", 1000, &[event(60_000)]).is_err());
    assert_eq!(stored(&outbox, 1), original);
    assert_eq!(outbox.dropped().unwrap(), 0);
    outbox.db.execute_batch("DROP TRIGGER fail_evict").unwrap();
    outbox.enqueue("synthetic", 1000, &[event(60_000)]).unwrap();
    assert!(outbox
        .declarations
        .event
        .get_in_connection(&outbox.db, EVENT_DEFINITION, &event_ref(1))
        .is_err());
    assert_eq!(outbox.dropped().unwrap(), 1);
    assert!(outbox.accounting().unwrap().bytes <= MAX_QUEUE_BYTES as u64);
    super::accounting_tests::assert_totals(&outbox);
}
#[test]
fn rejected_oversize_event_does_not_evict_a_full_count_queue() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    let mut outbox = open(&path).unwrap();
    for _ in 0..40 {
        outbox
            .enqueue(
                "synthetic",
                1000,
                &(0..250).map(|_| event(0)).collect::<Vec<_>>(),
            )
            .unwrap();
    }
    assert_eq!(outbox.accounting().unwrap().rows, 10_000);
    super::accounting_tests::assert_totals(&outbox);
    let original = stored(&outbox, 1);
    outbox
        .enqueue("synthetic", 1000, &[event(100_000)])
        .unwrap();
    assert_eq!(stored(&outbox, 1), original);
    assert_eq!(outbox.dropped().unwrap(), 1);
    outbox.enqueue("synthetic", 1000, &[event(0)]).unwrap();
    assert_eq!(outbox.accounting().unwrap().rows, 10_000);
    super::accounting_tests::assert_totals(&outbox);
    assert_eq!(outbox.dropped().unwrap(), 2);
    assert_eq!(
        stored(&outbox, 10_001).value.properties["record"]["sequence"],
        10_001
    );
}
#[test]
fn repeated_enqueue_ack_purge_has_flat_database_and_no_wal_growth() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    let mut outbox = open(&path).unwrap();
    let mut baseline = 0;
    for round in 0..100 {
        outbox.enqueue("synthetic", 1000, &[event(8192)]).unwrap();
        let rows = outbox.pending("synthetic", 100).unwrap();
        outbox.acknowledge(&[rows[0].0.clone()]).unwrap();
        let size = std::fs::metadata(path.join("queue.sqlite3")).unwrap().len();
        if round == 10 {
            baseline = size;
        }
        if round > 10 {
            assert_eq!(size, baseline);
        }
        assert!(!path.join("queue.sqlite3-wal").exists());
        assert_eq!(
            outbox
                .db
                .query_row("SELECT count(*) FROM ref_bindings", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            5
        );
    }
    assert_eq!(outbox.accounting().unwrap().rows, 0);
}

#[test]
fn escaped_payload_batch_fits_both_http_and_atomic_purge_byte_budgets() {
    let temp = tempfile::tempdir().unwrap();
    let mut outbox = open(&temp.path().join("outbox")).unwrap();
    let events: Vec<_> = (0..50)
        .map(|_| {
            let mut value = event(0);
            value["properties"]["padding"] = json!("\"\\\n".repeat(5000));
            value
        })
        .collect();
    outbox.enqueue("synthetic", 1000, &events).unwrap();
    let mut acknowledged = 0;
    loop {
        let rows = outbox.pending("synthetic", 100).unwrap();
        if rows.is_empty() {
            break;
        }
        let size: usize = rows
            .iter()
            .map(|(token, _)| {
                serde_json::to_vec(
                    &outbox
                        .declarations
                        .event
                        .get_in_connection(&outbox.db, EVENT_DEFINITION, &token.reference)
                        .unwrap()
                        .value
                        .properties,
                )
                .unwrap()
                .len()
            })
            .sum();
        assert!(size <= 1_048_576);
        acknowledged += rows.len();
        outbox
            .acknowledge(&rows.into_iter().map(|(token, _)| token).collect::<Vec<_>>())
            .unwrap();
    }
    assert_eq!(acknowledged, 50);
}
#[test]
fn signed_legacy_receipts_are_preserved_and_expire_under_the_owner_cutoff() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    let db = security::open(&path).unwrap();
    db.execute_batch(include_str!("legacy_fixture.sql"))
        .unwrap();
    db.execute("UPDATE queue SET received=-5 WHERE id=1", [])
        .unwrap();
    drop(db);
    let mut outbox = open(&path).unwrap();
    assert_eq!(
        stored(&outbox, 1).value.properties["record"]["received"],
        -5
    );
    assert_eq!(outbox.expire(0).unwrap(), 1);
    assert_eq!(outbox.dropped().unwrap(), 18);
    assert_eq!(outbox.accounting().unwrap().rows, 2);
}
