use super::*;
use migration::{convert_with_checkpoint, Stage};
use rusqlite::OptionalExtension;

fn legacy(db: &Connection) {
    db.execute_batch(include_str!("legacy_fixture.sql"))
        .unwrap();
}
fn snapshot(db: &Connection) -> Vec<(i64, String, i64, String, i64)> {
    db.prepare("SELECT id,destination,received,event,quarantined FROM queue ORDER BY id")
        .unwrap()
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn open_legacy(path: &Path) -> Connection {
    let db = security::open(path).unwrap();
    legacy(&db);
    db
}

#[test]
fn converts_exact_retained_strings_identity_counters_and_deleted_high_water() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let db = open_legacy(&path);
    let before = snapshot(&db);
    drop(db);
    let outbox = open(&path).unwrap();
    assert_eq!(
        outbox.migration,
        MigrationReport {
            imported: 3,
            high_water: 20,
            repeated: false
        }
    );
    assert_eq!(
        outbox.installation_id().unwrap(),
        "00000000-0000-4000-8000-000000000001"
    );
    for (id, destination, received, event, quarantined) in &before {
        let record = outbox
            .declarations
            .event
            .get_in_connection(&outbox.db, EVENT_DEFINITION, &event_ref(*id))
            .unwrap();
        assert_eq!(
            record.value.properties,
            event_value(
                *id,
                *received,
                destination.clone(),
                event.clone(),
                *quarantined == 1
            )
        );
    }
    let state = outbox
        .declarations
        .state
        .get_in_connection(&outbox.db, STATE_DEFINITION, STATE_REF)
        .unwrap();
    assert_eq!(state.value.properties["record"]["counters"]["dropped"], 17);
    assert_eq!(
        state.value.properties["record"]["counters"]["synthetic_extra"],
        5
    );
    assert!(!outbox
        .db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='queue')",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    drop(outbox);
    let reopened = open(&path).unwrap();
    assert_eq!(
        reopened.migration,
        MigrationReport {
            imported: 0,
            high_water: 20,
            repeated: true
        }
    );
    assert_eq!(
        reopened
            .declarations
            .state
            .get_in_connection(&reopened.db, STATE_DEFINITION, STATE_REF)
            .unwrap(),
        state
    );
    assert_eq!(
        reopened
            .db
            .query_row(
                "SELECT count(*) FROM nodes WHERE kind='posthog_event'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        3
    );
}

#[test]
fn every_interrupted_startup_stage_rolls_back_and_reopens_legacy() {
    for failure in [
        Stage::Initialized,
        Stage::Imported(1),
        Stage::Imported(2),
        Stage::Imported(3),
        Stage::StateCreated,
        Stage::LegacyRetired,
        Stage::IndexesInstalled,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outbox");
        let mut db = open_legacy(&path);
        let before = snapshot(&db);
        let result = convert_with_checkpoint(&mut db, &test_declarations().unwrap(), |stage| {
            if stage == failure {
                Err("injected startup failure".into())
            } else {
                Ok(())
            }
        });
        assert!(result.is_err(), "{failure:?}");
        drop(db);
        let db = security::open(&path).unwrap();
        assert_eq!(snapshot(&db), before, "{failure:?}");
        assert_eq!(
            db.query_row("SELECT value FROM counters WHERE key='dropped'", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            17
        );
        assert_eq!(
            db.query_row(
                "SELECT seq FROM sqlite_sequence WHERE name='queue'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            20
        );
        assert!(!db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='nodes')",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        drop(db);
        assert_eq!(open(&path).unwrap().migration.imported, 3);
    }
}

#[test]
fn fresh_startup_repeats_without_resetting_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let first = open(&path).unwrap();
    assert_eq!(first.migration.high_water, 0);
    let identity = first.installation_id().unwrap();
    drop(first);
    let second = open(&path).unwrap();
    assert!(second.migration.repeated);
    assert_eq!(second.installation_id().unwrap(), identity);
}

#[test]
fn invalid_legacy_state_never_resets_identity_or_retires_source() {
    for sql in [
        "DELETE FROM identity",
        "DELETE FROM counters WHERE key='dropped'",
        "UPDATE queue SET quarantined=2 WHERE id=1",
        "DROP TABLE counters",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outbox");
        let db = open_legacy(&path);
        db.execute_batch(sql).unwrap();
        let before = snapshot(&db);
        drop(db);
        assert!(open(&path).is_err(), "{sql}");
        let db = security::open(&path).unwrap();
        assert_eq!(snapshot(&db), before);
        assert_eq!(
            db.query_row(
                "SELECT name FROM sqlite_master WHERE name='nodes'",
                [],
                |r| r.get::<_, String>(0)
            )
            .optional()
            .unwrap(),
            None
        );
    }
}

#[cfg(unix)]
#[test]
fn retained_file_security_rejects_symlinks_shared_modes_and_hardlinks() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let outbox = open(&path).unwrap();
    let database = path.join("queue.sqlite3");
    drop(outbox);
    std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(open(&path).is_err());
    std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o600)).unwrap();
    let hardlink = path.join("shared.sqlite3");
    std::fs::hard_link(&database, &hardlink).unwrap();
    assert!(open(&path).is_err());
    std::fs::remove_file(&hardlink).unwrap();
    let retained = path.join("retained.sqlite3");
    std::fs::rename(&database, &retained).unwrap();
    symlink(&retained, &database).unwrap();
    assert!(open(&path).is_err());
    std::fs::remove_file(&database).unwrap();
    std::fs::rename(&retained, &database).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(open(&path).is_err());
}

#[test]
fn process_exit_before_commit_recovers_legacy_journal() {
    for crash_at in [
        "Initialized",
        "Imported(1)",
        "Imported(3)",
        "StateCreated",
        "LegacyRetired",
        "IndexesInstalled",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outbox");
        let db = open_legacy(&path);
        let before = snapshot(&db);
        drop(db);
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "posthog::migration_tests::migration_crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env("CWR_TEST_OUTBOX_CRASH_PATH", &path)
            .env("CWR_TEST_OUTBOX_CRASH_STAGE", crash_at)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(91),
            "{crash_at}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let db = security::open(&path).unwrap();
        assert_eq!(snapshot(&db), before);
        assert_eq!(
            db.query_row(
                "SELECT seq FROM sqlite_sequence WHERE name='queue'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            20
        );
        drop(db);
        let recovered = open(&path).unwrap();
        assert_eq!(recovered.migration.imported, 3);
        assert_eq!(recovered.migration.high_water, 20);
    }
}

#[test]
#[ignore = "subprocess crash fixture invoked by process_exit_before_commit_recovers_legacy_journal"]
fn migration_crash_child() {
    let path = std::env::var_os("CWR_TEST_OUTBOX_CRASH_PATH").expect("child fixture path");
    let crash_at = std::env::var("CWR_TEST_OUTBOX_CRASH_STAGE").unwrap();
    let mut db = security::open(Path::new(&path)).unwrap();
    convert_with_checkpoint(&mut db, &test_declarations().unwrap(), |stage| {
        if format!("{stage:?}") == crash_at {
            std::process::exit(91);
        }
        Ok(())
    })
    .unwrap();
    panic!("fixture did not reach crash point");
}

#[test]
fn sqlite_full_conversion_keeps_old_tables_and_retries_after_capacity_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut db = open_legacy(&path);
    let before = snapshot(&db);
    let pages: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    db.pragma_update(None, "max_page_count", pages).unwrap();
    let error = migration::convert(&mut db, &test_declarations().unwrap()).unwrap_err();
    assert!(
        error
            .downcast_ref::<rusqlite::Error>()
            .is_some_and(|e| e.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull)),
        "{error}"
    );
    assert_eq!(snapshot(&db), before);
    db.pragma_update(None, "max_page_count", 4096).unwrap();
    assert_eq!(
        migration::convert(&mut db, &test_declarations().unwrap())
            .unwrap()
            .imported,
        3
    );
}

#[test]
fn conversion_reuses_legacy_pages_without_doubling_database_quota() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut db = open_legacy(&path);
    db.execute("DELETE FROM queue", []).unwrap();
    let body = "x".repeat(60_000);
    {
        let tx = db.transaction().unwrap();
        for index in 1..=96 {
            tx.execute(
                "INSERT INTO queue(id,destination,received,event) VALUES(?1,'synthetic',1000,?2)",
                rusqlite::params![index, &body],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let pages: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    db.pragma_update(None, "max_page_count", pages + 128)
        .unwrap();
    let report = migration::convert(&mut db, &test_declarations().unwrap()).unwrap();
    assert_eq!(report.imported, 96);
    assert_eq!(report.high_water, 96);
    let current: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    assert!(current <= pages + 128);
}

#[cfg(unix)]
#[test]
fn disappearing_or_replaced_source_cannot_initialize_an_empty_replacement() {
    use std::os::unix::fs::PermissionsExt;
    for replace in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outbox");
        let db = open_legacy(&path);
        let before = snapshot(&db);
        drop(db);
        let database = path.join("queue.sqlite3");
        let retained = path.join("retained.sqlite3");
        let result = security::open_with_checkpoint(&path, || {
            std::fs::rename(&database, &retained)?;
            if replace {
                std::fs::write(&database, [])?;
                std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o600))?;
            }
            Ok(())
        });
        assert!(result.is_err());
        if replace {
            assert_eq!(std::fs::metadata(&database).unwrap().len(), 0);
            std::fs::remove_file(&database).unwrap();
        } else {
            assert!(!database.exists());
        }
        std::fs::rename(&retained, &database).unwrap();
        let db = security::open(&path).unwrap();
        assert_eq!(snapshot(&db), before);
        drop(db);
        assert_eq!(open(&path).unwrap().migration.imported, 3);
    }
}

#[test]
fn unknown_retained_columns_are_rejected_without_losing_their_values() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("outbox");
    let mut db = open_legacy(&path);
    db.execute_batch("ALTER TABLE queue ADD COLUMN retained_extension TEXT; UPDATE queue SET retained_extension='synthetic retained extension' WHERE id=1;").unwrap();
    let before = snapshot(&db);
    assert!(migration::convert(&mut db, &test_declarations().unwrap()).is_err());
    assert_eq!(snapshot(&db), before);
    assert_eq!(
        db.query_row("SELECT retained_extension FROM queue WHERE id=1", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "synthetic retained extension"
    );
}
