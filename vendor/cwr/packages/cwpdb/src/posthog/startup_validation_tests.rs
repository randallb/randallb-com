//! Retained corruption must never disappear behind a sequence read cursor.
use super::*;
fn snapshot(db: &Connection) -> Vec<(String, String, String, i64)> {
    db.prepare("SELECT b.canonical_ref,n.kind,json(n.props),n.revision FROM nodes n JOIN ref_bindings b ON b.ref_id=n.ref_id UNION ALL SELECT b.canonical_ref,'edge:'||e.kind,json(e.props),e.revision FROM edges e JOIN ref_bindings b ON b.ref_id=e.ref_id ORDER BY 1,2")
        .unwrap().query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))
        .unwrap().collect::<Result<_,_>>().unwrap()
}
fn retained(path: &Path) -> PosthogOutbox {
    let db = security::open(path).unwrap();
    db.execute_batch(include_str!("legacy_fixture.sql"))
        .unwrap();
    drop(db);
    open(path).unwrap()
}
fn reject_unchanged(mut outbox: PosthogOutbox, path: &Path) {
    outbox.db.execute("UPDATE nodes SET props=jsonb_remove(props,'$.record.live_rows','$.record.live_bytes') WHERE kind='posthog_state'",[]).unwrap();
    // Force index recreation before validation to prove its DDL also rolls back.
    outbox
        .db
        .execute_batch("DROP INDEX posthog_pending_v1")
        .unwrap();
    let before = snapshot(&outbox.db);
    assert!(migration::convert(&mut outbox.db, &outbox.declarations).is_err());
    assert_eq!(snapshot(&outbox.db), before);
    assert_eq!(
        outbox
            .db
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='posthog_pending_v1'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    drop(outbox);
    assert!(open(path).is_err());
    let db = security::open(path).unwrap();
    assert_eq!(snapshot(&db), before);
}
#[test]
fn startup_rejects_sequences_outside_the_positive_keyset_and_preserves_source() {
    for expression in [
        "jsonb_set(props,'$.record.sequence',0)",
        "jsonb_set(props,'$.record.sequence',-1)",
        "jsonb_remove(props,'$.record.sequence')",
        "jsonb_set(props,'$.record.sequence','invalid')",
        "jsonb_set(props,'$.record.sequence',1.5)",
        "jsonb_set(props,'$.record.sequence',NULL)",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outbox");
        let outbox = retained(&path);
        outbox
            .db
            .execute(
                &format!("UPDATE nodes SET props={expression} WHERE kind='posthog_event'"),
                [],
            )
            .unwrap();
        reject_unchanged(outbox, &path);
    }
}
#[test]
fn startup_rejects_duplicate_positive_sequence_beyond_old_page_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut outbox = retained(&path);
    let tx = outbox.db.transaction().unwrap();
    for sequence in 21..=271 {
        outbox
            .declarations
            .event
            .create_in_transaction(
                &tx,
                EVENT_DEFINITION,
                &event_ref(sequence),
                &event_value(sequence, 1000, "synthetic".into(), "{}".into(), false),
            )
            .unwrap();
    }
    tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.record.high_water',271) WHERE kind='posthog_state'",[]).unwrap();
    tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.record.sequence',267) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[event_ref(268)]).unwrap();
    tx.commit().unwrap();
    reject_unchanged(outbox, &path);
}
#[test]
fn startup_rejects_malformed_envelope_bytes_and_mismatched_domain() {
    for (column,expression) in [
        ("props","jsonb_remove(props,'$.record')"),
        ("props","jsonb_set(props,'$.record',0)"),
        ("props","jsonb_set(props,'$.record.bytes',0)"),
        ("kind","'wrong_kind'"),
        ("schema_id","(SELECT ref_id FROM ref_bindings WHERE canonical_ref='cwp://org.coworkerprotocol/cwr/entities/posthog-state')"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outbox");
        let outbox = retained(&path);
        outbox.db.execute(&format!("UPDATE nodes SET {column}={expression} WHERE kind='posthog_event'"),[]).unwrap();
        reject_unchanged(outbox,&path);
    }
}

#[test]
fn startup_rejects_event_schema_on_an_edge_without_removing_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let outbox = retained(&path);
    outbox.db.execute_batch("INSERT INTO edges(ref_id,kind,schema_id,source_id,target_id,props,revision) SELECT ref_id,kind,schema_id,ref_id,ref_id,props,revision FROM nodes WHERE kind='posthog_event' LIMIT 1;").unwrap();
    reject_unchanged(outbox, &path);
}
