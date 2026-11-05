use super::*;
use crate::ref_read::RefRecord;
fn fixture(path: &Path) -> PosthogOutbox {
    let db = security::open(path).unwrap();
    db.execute_batch(include_str!("legacy_fixture.sql"))
        .unwrap();
    drop(db);
    open(path).unwrap()
}
fn record(outbox: &PosthogOutbox, id: i64) -> RefRecord {
    outbox
        .declarations
        .event
        .get_in_connection(&outbox.db, EVENT_DEFINITION, &event_ref(id))
        .unwrap()
}
#[test]
fn acknowledge_sql_failure_rolls_back_the_entire_selected_batch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut outbox = fixture(&path);
    let first = record(&outbox, 1);
    let second = record(&outbox, 9);
    outbox.db.execute_batch("CREATE TEMP TRIGGER fail_ack BEFORE DELETE ON nodes WHEN OLD.ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref='cwp://org.coworkerprotocol/cwr/posthog/events/9') BEGIN SELECT RAISE(ABORT,'injected acknowledgement failure'); END;").unwrap();
    assert!(outbox
        .acknowledge(&[Selection::from(&first), Selection::from(&second)])
        .is_err());
    assert_eq!(record(&outbox, 1), first);
    assert_eq!(record(&outbox, 9), second);
    drop(outbox);
    let mut outbox = open(&path).unwrap();
    assert_eq!(record(&outbox, 1), first);
    assert_eq!(record(&outbox, 9), second);
    outbox
        .acknowledge(&[Selection::from(&first), Selection::from(&second)])
        .unwrap();
    assert!(outbox
        .declarations
        .event
        .get_in_connection(&outbox.db, EVENT_DEFINITION, &event_ref(1))
        .is_err());
    assert!(outbox
        .declarations
        .event
        .get_in_connection(&outbox.db, EVENT_DEFINITION, &event_ref(9))
        .is_err());
    assert_eq!(
        record(&outbox, 4).value.properties["record"]["quarantined"],
        true
    );
}
#[test]
fn quarantine_survives_reopen_without_changing_original_event() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut outbox = fixture(&path);
    let original = record(&outbox, 9);
    outbox.quarantine(&[Selection::from(&original)]).unwrap();
    assert!(outbox.acknowledge(&[Selection::from(&original)]).is_err());
    drop(outbox);
    let mut reopened = open(&path).unwrap();
    let stored = record(&reopened, 9);
    let mut expected = original.value.properties;
    expected["record"]["quarantined"] = json!(true);
    assert_eq!(stored.value.properties, expected);
    let revision = stored.revision.clone();
    reopened.quarantine(&[Selection::from(&stored)]).unwrap();
    assert_eq!(record(&reopened, 9).revision, revision);
}
#[test]
fn admission_committed_during_http_window_is_not_acknowledged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut outbox = fixture(&path);
    let selected = outbox.pending("synthetic-destination-a", 100).unwrap()[0]
        .0
        .clone();
    // Another actual outbox owner commits while the selected HTTP batch is out.
    let join = std::thread::spawn({
        let path = path.clone();
        move || {
            let mut writer = open(&path).unwrap();
            writer.enqueue("synthetic-destination-a",5000,&[json!({"uuid":uuid::Uuid::new_v4().to_string(),"timestamp":"2026-10-23T00:00:00Z","event":"cwr_metric","properties":{}})]).unwrap();
        }
    });
    join.join().unwrap();
    outbox.acknowledge(&[selected]).unwrap();
    assert_eq!(
        record(&outbox, 21).value.properties["record"]["sequence"],
        21
    );
    assert_eq!(open(&path).unwrap().migration.high_water, 21);
}
