//! Physical-index exception evidence. These are isolated synthetic stores.
use super::*;
use crate::sequence_read;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};
fn legacy_rows(path: &Path, count: i64) -> Connection {
    let mut db = security::open(path).unwrap();
    db.execute_batch(include_str!("legacy_fixture.sql"))
        .unwrap();
    let tx = db.transaction().unwrap();
    tx.execute("DELETE FROM queue", []).unwrap();
    let body =
        json!({"uuid":"synthetic-retained","properties":{"padding":"x".repeat(1024)}}).to_string();
    for sequence in 1..=count {
        tx.execute(
            "INSERT INTO queue(id,destination,received,event) VALUES(?1,'synthetic',1000,?2)",
            rusqlite::params![sequence, &body],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    db
}
#[test]
fn index_build_disk_full_rolls_back_source_and_retries_with_identical_rows() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    let mut db = legacy_rows(&path, 1000);
    migration::convert(&mut db, &test_declarations().unwrap()).unwrap();
    // Compact away free source pages so quota failure is inside index creation.
    db.execute_batch("DROP INDEX posthog_admission_v1;DROP INDEX posthog_pending_v1;VACUUM;")
        .unwrap();
    let pages: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    db.pragma_update(None, "max_page_count", pages).unwrap();
    let error = migration::convert(&mut db, &test_declarations().unwrap()).unwrap_err();
    assert!(
        error
            .downcast_ref::<rusqlite::Error>()
            .is_some_and(|e| e.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull)),
        "{error}"
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM nodes WHERE kind='posthog_event'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1000
    );
    assert_eq!(db.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('posthog_admission_v1','posthog_pending_v1')",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    db.pragma_update(None, "max_page_count", 4096).unwrap();
    assert!(
        migration::convert(&mut db, &test_declarations().unwrap())
            .unwrap()
            .repeated
    );
    assert_eq!(
        reads::accounting(&db, &test_declarations().unwrap())
            .unwrap()
            .rows,
        1000
    );
}
#[test]
fn conflicting_retained_index_never_resets_or_reimports_the_queue() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    let mut db = legacy_rows(&path, 3);
    db.execute_batch("CREATE INDEX posthog_pending_v1 ON queue(destination);")
        .unwrap();
    assert!(migration::convert(&mut db, &test_declarations().unwrap()).is_err());
    assert_eq!(
        db.query_row("SELECT count(*) FROM queue", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    db.execute_batch("DROP INDEX posthog_pending_v1").unwrap();
    assert_eq!(
        migration::convert(&mut db, &test_declarations().unwrap())
            .unwrap()
            .imported,
        3
    );
}
#[test]
fn measure_index_migration_and_graph_projection_alternative() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    let mut db = legacy_rows(&path, 1000);
    let before = std::fs::metadata(path.join("queue.sqlite3")).unwrap().len();
    let started = Instant::now();
    let mut journal_peak = 0u64;
    let report = migration::convert_with_checkpoint(&mut db, &test_declarations().unwrap(), |_| {
        journal_peak = journal_peak.max(
            std::fs::metadata(path.join("queue.sqlite3-journal"))
                .map(|m| m.len())
                .unwrap_or(0),
        );
        Ok(())
    })
    .unwrap();
    let migration_us = started.elapsed().as_micros();
    assert_eq!(report.imported, 1000);
    assert_eq!(
        db.query_row("PRAGMA temp_store", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    let compile_temp:String=db.query_row("SELECT compile_options FROM pragma_compile_options WHERE compile_options LIKE 'TEMP_STORE=%'",[],|r|r.get(0)).unwrap();
    assert_ne!(compile_temp, "TEMP_STORE=0");
    let mut snapshot = Vec::new();
    {
        let mut after = None;
        loop {
            let page = reads::admission_index()
                .read(
                    &db,
                    &[],
                    sequence_read::Direction::Ascending,
                    after,
                    None,
                    sequence_read::ReadBounds {
                        rows: 250,
                        bytes: 1_048_576,
                        work: 10_000,
                    },
                )
                .unwrap();
            snapshot.extend(page.items.into_iter().map(|r| (r.reference, r.sequence)));
            if page.complete {
                break;
            }
            after = page.next;
        }
    }
    // Both alternatives start from the identical declared authoritative events.
    db.execute_batch("DROP INDEX posthog_admission_v1;DROP INDEX posthog_pending_v1;VACUUM;")
        .unwrap();
    drop(db);
    let graph_path = temp.path().join("graph-alternative");
    std::fs::create_dir(&graph_path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&graph_path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::copy(path.join("queue.sqlite3"), graph_path.join("queue.sqlite3")).unwrap();
    let mut db = security::open(&path).unwrap();
    let index_steps = Arc::new(AtomicU64::new(0));
    let steps = index_steps.clone();
    db.progress_handler(
        1,
        Some(move || {
            steps.fetch_add(1, Ordering::Relaxed);
            false
        }),
    )
    .unwrap();
    let started = Instant::now();
    {
        let tx = db.transaction().unwrap();
        reads::install(&tx).unwrap();
        tx.commit().unwrap();
    }
    let index_us = started.elapsed().as_micros();
    db.progress_handler(0, None::<fn() -> bool>).unwrap();
    db.execute_batch("VACUUM").unwrap();
    let index_size = std::fs::metadata(path.join("queue.sqlite3")).unwrap().len();
    let mut graph = security::open(&graph_path).unwrap();
    let graph_steps = Arc::new(AtomicU64::new(0));
    let steps = graph_steps.clone();
    graph
        .progress_handler(
            1,
            Some(move || {
                steps.fetch_add(1, Ordering::Relaxed);
                false
            }),
        )
        .unwrap();
    let started = Instant::now();
    {
        let tx = graph.transaction().unwrap();
        for (reference, sequence) in &snapshot {
            cwpdb::put_disposable_edge(
                &tx,
                "cwp://synthetic/outbox/admissions",
                "outbox_admission_projection",
                reference,
                *sequence,
                &json!({}),
            )
            .unwrap();
            cwpdb::put_disposable_edge(
                &tx,
                "cwp://synthetic/outbox/pending",
                "outbox_pending_projection",
                reference,
                *sequence,
                &json!({}),
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let graph_us = started.elapsed().as_micros();
    graph.progress_handler(0, None::<fn() -> bool>).unwrap();
    graph.execute_batch("VACUUM").unwrap();
    let graph_size = std::fs::metadata(graph_path.join("queue.sqlite3"))
        .unwrap()
        .len();
    let graph_edges: i64 = graph
        .query_row("SELECT count(*) FROM edges", [], |r| r.get(0))
        .unwrap();
    assert_eq!(graph_edges, 2000);
    println!(
        "outbox_architecture_evidence {}",
        json!({"events":1000,"payload_bytes_each":1024,"source_db_bytes":before,"migration_us":migration_us,"temp_sort_disk_bytes_by_verified_policy":0,"sqlite_temp_compile_option":compile_temp,"rollback_journal_peak_observed_bytes":journal_peak,"temp_sort_policy":"MEMORY; rollback journal remains required","expression_index_entries":2000,"expression_build_vm_steps":index_steps.load(Ordering::Relaxed),"expression_build_instrumented_us":index_us,"expression_db_bytes":index_size,"graph_projection_edges":graph_edges,"graph_projection_vm_steps":graph_steps.load(Ordering::Relaxed),"graph_build_instrumented_us":graph_us,"graph_db_bytes":graph_size,"scope":"synthetic startup projection construction; not general runtime latency"})
    );
}
