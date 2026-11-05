use super::*;
fn event() -> Value {
    json!({"uuid":uuid::Uuid::new_v4().to_string(),"timestamp":"2026-10-23T00:00:00Z",
        "event":"synthetic","properties":{"utf8":"雪🌻"}})
}
pub(super) fn assert_totals(outbox: &PosthogOutbox) {
    let expected: (i64,i64) = outbox.db.query_row(
        "SELECT count(*),coalesce(sum(length(CAST(json_extract(props,'$.record.event') AS BLOB))),0) FROM nodes WHERE kind='posthog_event'",
        [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    let actual = outbox.accounting().unwrap();
    assert_eq!((actual.rows as i64, actual.bytes as i64), expected);
}
#[test]
fn missing_totals_rebuild_original_utf8_and_quarantine_without_double_counting() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut outbox = open(&path).unwrap();
    outbox
        .enqueue("synthetic", 1000, &[event(), event()])
        .unwrap();
    let rows = outbox.pending("synthetic", 100).unwrap();
    outbox.quarantine(&[rows[0].0.clone()]).unwrap();
    outbox.db.execute("UPDATE nodes SET props=jsonb_remove(props,'$.record.live_rows','$.record.live_bytes') WHERE kind='posthog_state'",[]).unwrap();
    drop(outbox);
    for _ in 0..2 {
        let outbox = open(&path).unwrap();
        assert_eq!(outbox.accounting().unwrap().rows, 2);
        assert_totals(&outbox);
    }
}
#[test]
fn stale_partial_or_corrupt_retained_totals_fail_closed() {
    for expression in [
        "jsonb_set(props,'$.record.live_rows',0,'$.record.live_bytes',0)",
        "jsonb_remove(props,'$.record.live_bytes')",
        "jsonb_set(props,'$.record.live_bytes','corrupt')",
        "jsonb_set(props,'$.record.live_bytes',-1)",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outbox");
        let mut outbox = open(&path).unwrap();
        outbox.enqueue("synthetic", 1000, &[event()]).unwrap();
        outbox
            .db
            .execute(
                &format!("UPDATE nodes SET props={expression} WHERE kind='posthog_state'"),
                [],
            )
            .unwrap();
        let before: String = outbox
            .db
            .query_row(
                "SELECT json(props) FROM nodes WHERE kind='posthog_state'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        drop(outbox);
        assert!(open(&path).is_err());
        let db = security::open(&path).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT json(props) FROM nodes WHERE kind='posthog_state'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            before
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM nodes WHERE kind='posthog_event'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
}
#[test]
fn rebuild_rejects_incorrect_original_byte_field() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut outbox = open(&path).unwrap();
    outbox.enqueue("synthetic", 1000, &[event()]).unwrap();
    outbox.db.execute("UPDATE nodes SET props=jsonb_remove(props,'$.record.live_rows','$.record.live_bytes') WHERE kind='posthog_state'",[]).unwrap();
    outbox
        .db
        .execute(
            "UPDATE nodes SET props=jsonb_set(props,'$.record.bytes',1) WHERE kind='posthog_event'",
            [],
        )
        .unwrap();
    drop(outbox);
    assert!(open(&path).is_err());
}
#[test]
fn aggregate_write_failure_rolls_back_append_ack_and_high_water() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut outbox = open(&path).unwrap();
    outbox.enqueue("synthetic", 1000, &[event()]).unwrap();
    let selected = outbox.pending("synthetic", 100).unwrap()[0].0.clone();
    let original = outbox
        .declarations
        .state
        .get_in_connection(&outbox.db, STATE_DEFINITION, STATE_REF)
        .unwrap();
    outbox.db.execute_batch("CREATE TEMP TRIGGER fail_totals BEFORE UPDATE ON nodes WHEN OLD.kind='posthog_state' AND json_extract(NEW.props,'$.record.live_rows') != json_extract(OLD.props,'$.record.live_rows') BEGIN SELECT RAISE(ABORT,'aggregate failure'); END;").unwrap();
    assert!(outbox.enqueue("synthetic", 1000, &[event()]).is_err());
    assert!(outbox.acknowledge(std::slice::from_ref(&selected)).is_err());
    assert_eq!(
        outbox
            .declarations
            .state
            .get_in_connection(&outbox.db, STATE_DEFINITION, STATE_REF)
            .unwrap(),
        original
    );
    assert_totals(&outbox);
    outbox.db.execute_batch("DROP TRIGGER fail_totals").unwrap();
    outbox.acknowledge(&[selected]).unwrap();
    assert_eq!(outbox.accounting().unwrap().rows, 0);
    assert_totals(&outbox);
    outbox.enqueue("synthetic", 1000, &[event()]).unwrap();
    assert_eq!(outbox.accounting().unwrap().max_live_sequence, 2);
    drop(outbox);
    assert_totals(&open(&path).unwrap());
}
#[test]
fn interleaved_owner_insert_and_ack_keep_exact_totals() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut outbox = open(&path).unwrap();
    outbox.enqueue("synthetic", 1000, &[event()]).unwrap();
    let selected = outbox.pending("synthetic", 100).unwrap()[0].0.clone();
    let writer_path = path.clone();
    std::thread::spawn(move || {
        let mut writer = open(&writer_path).unwrap();
        writer.enqueue("synthetic", 1000, &[event()]).unwrap();
    })
    .join()
    .unwrap();
    outbox.acknowledge(&[selected]).unwrap();
    assert_eq!(outbox.accounting().unwrap().rows, 1);
    assert_totals(&outbox);
    drop(outbox);
    assert_totals(&open(&path).unwrap());
}
