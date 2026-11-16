use super::*;
use crate::v2026_09::conversation::ConversationAdmissionStore;

#[path = "read_tests/summary_tests.rs"]
mod summary_tests;

#[path = "read_tests/stdlib_paging.rs"]
mod stdlib_paging;

#[path = "read_tests/legacy_module_paging.rs"]
mod legacy_module_paging;

const RUNTIME: &str = "cwp://com.example.runtime";
const TIMEOUT: Duration = Duration::from_secs(2);

fn seeded_store(count: usize) -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite");
    drop(ConversationAdmissionStore::open(&path, RUNTIME).unwrap());
    cwpdb_store::test_support::summary_pages::seed(&path, RUNTIME, count).unwrap();
    (temporary, path)
}

#[test]
fn pages_use_relay_windows_and_exact_page_info() {
    let (_temporary, path) = seeded_store(8);
    cwpdb_store::test_support::summary_pages::archive_first(&path, RUNTIME).unwrap();
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    let first = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(3),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(orders(&first), vec![7, 6, 5]);
    assert_eq!(first.total_count, 8);
    assert_eq!(first.archived_count, 1);
    assert!(first.has_next_page);
    assert!(!first.has_previous_page);
    let second = operation
        .root_conversations(ConversationSummaryWindow {
            after: first.summaries.last().map(|summary| &summary.key),
            first: Some(4),
            last: Some(2),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(orders(&second), vec![2, 1]);
    assert_eq!(second.total_count, 8);
    assert_eq!(second.archived_count, 1);
    assert!(second.has_next_page);
    assert!(second.has_previous_page);
    let previous = operation
        .root_conversations(ConversationSummaryWindow {
            before: second.summaries.first().map(|summary| &summary.key),
            last: Some(3),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(orders(&previous), vec![5, 4, 3]);
    assert_eq!(previous.total_count, 8);
    assert!(!previous.has_next_page);
    assert!(previous.has_previous_page);
}

#[test]
fn exact_boundaries_do_not_claim_pages_from_cursor_presence() {
    let (_temporary, path) = seeded_store(1);
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    let only = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(1),
            ..Default::default()
        })
        .unwrap()
        .summaries
        .pop()
        .unwrap();
    for page in [
        operation
            .root_conversations(ConversationSummaryWindow {
                after: Some(&only.key),
                first: Some(1),
                ..Default::default()
            })
            .unwrap(),
        operation
            .root_conversations(ConversationSummaryWindow {
                before: Some(&only.key),
                last: Some(1),
                ..Default::default()
            })
            .unwrap(),
    ] {
        assert!(page.summaries.is_empty());
        assert!(!page.has_next_page);
        assert!(!page.has_previous_page);
    }
}

#[test]
fn cursor_edges_are_excluded_from_nonempty_page_info() {
    let (_temporary, path) = seeded_store(3);
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    let all = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(3),
            ..Default::default()
        })
        .unwrap();
    let top = &all.summaries[0].key;
    let bottom = &all.summaries[2].key;

    let after_top = operation
        .root_conversations(ConversationSummaryWindow {
            after: Some(top),
            first: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(orders(&after_top), vec![1]);
    assert!(!after_top.has_previous_page);
    assert!(after_top.has_next_page);

    let before_bottom = operation
        .root_conversations(ConversationSummaryWindow {
            before: Some(bottom),
            last: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(orders(&before_bottom), vec![1]);
    assert!(before_bottom.has_previous_page);
    assert!(!before_bottom.has_next_page);

    let bounded = operation
        .root_conversations(ConversationSummaryWindow {
            after: Some(top),
            before: Some(bottom),
            first: Some(1),
            last: Some(1),
        })
        .unwrap();
    assert_eq!(orders(&bounded), vec![1]);
    assert!(!bounded.has_previous_page);
    assert!(!bounded.has_next_page);
}

#[test]
fn operation_holds_one_wal_snapshot_and_honors_cancellation() {
    let (_temporary, path) = seeded_store(2);
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    let first = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(10),
            ..Default::default()
        })
        .unwrap();
    cwpdb_store::test_support::summary_pages::append(&path, RUNTIME, 2).unwrap();
    let same_snapshot = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(10),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(orders(&first), orders(&same_snapshot));
    let cancellation = operation.cancellation();
    cancellation.cancel();
    assert!(operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(1),
            ..Default::default()
        })
        .is_err());
}

#[test]
fn cancellation_during_registration_prevents_the_initial_snapshot_query() {
    let (_temporary, path) = seeded_store(1);
    cwpdb_store::test_support::summary_pages::rollback_journal(&path, RUNTIME).unwrap();
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let mut blocker =
        cwpdb_store::test_support::summary_pages::ExclusiveReadLock::acquire(&path, RUNTIME)
            .unwrap();
    let started = Instant::now();
    let result = store.begin_operation_until_with(Instant::now() + TIMEOUT, |cancellation| {
        cancellation.cancel();
    });
    let Err(error) = result else {
        panic!("cancellation during registration must prevent snapshot establishment")
    };
    assert!(error.to_string().contains("cancelled"));
    assert!(started.elapsed() < Duration::from_millis(100));
    blocker.release().unwrap();
}

#[test]
fn initial_snapshot_retries_transient_locks_within_the_deadline() {
    let (_temporary, path) = seeded_store(1);
    cwpdb_store::test_support::summary_pages::rollback_journal(&path, RUNTIME).unwrap();
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let mut blocker =
        cwpdb_store::test_support::summary_pages::ExclusiveReadLock::acquire(&path, RUNTIME)
            .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        blocker.release().unwrap();
    });

    let started = Instant::now();
    let operation = store.begin_operation(Duration::from_millis(500)).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(40));
    assert!(started.elapsed() < Duration::from_millis(200));
    assert!(operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(1),
            ..Default::default()
        })
        .is_ok());
    release.join().unwrap();
}

#[test]
fn deadline_applies_during_read_database_inspection() {
    let (_temporary, path) = seeded_store(1);
    cwpdb_store::test_support::summary_pages::rollback_journal(&path, RUNTIME).unwrap();
    let mut blocker =
        cwpdb_store::test_support::summary_pages::ExclusiveReadLock::acquire(&path, RUNTIME)
            .unwrap();
    let cancellation = ConversationReadCancellation::pending();
    let started = Instant::now();
    let result = ConversationReadStore::open_until(
        &path,
        RUNTIME,
        Instant::now() + Duration::from_millis(10),
        &cancellation,
    );
    let Err(error) = result else {
        panic!("database inspection must honor the read deadline")
    };
    assert!(error.to_string().contains("locked") || error.to_string().contains("deadline"));
    assert!(started.elapsed() < Duration::from_millis(100));
    blocker.release().unwrap();
}

#[test]
fn cancellation_interrupts_read_database_inspection_lock_retries() {
    let (_temporary, path) = seeded_store(1);
    cwpdb_store::test_support::summary_pages::rollback_journal(&path, RUNTIME).unwrap();
    let mut blocker =
        cwpdb_store::test_support::summary_pages::ExclusiveReadLock::acquire(&path, RUNTIME)
            .unwrap();
    let cancellation = ConversationReadCancellation::pending();
    let worker_cancellation = cancellation.clone();
    let worker_path = path.clone();
    let started = Instant::now();
    let worker = std::thread::spawn(move || {
        ConversationReadStore::open_until(
            worker_path,
            RUNTIME,
            Instant::now() + TIMEOUT,
            &worker_cancellation,
        )
        .err()
        .map(|error| error.to_string())
    });
    std::thread::sleep(Duration::from_millis(20));
    cancellation.cancel();
    let error = worker
        .join()
        .unwrap()
        .expect("inspection must be cancelled");
    assert!(error.contains("cancelled"));
    assert!(started.elapsed() < Duration::from_millis(100));
    blocker.release().unwrap();
}

#[test]
fn failed_setup_detaches_its_interrupt_handle() {
    let (_temporary, path) = seeded_store(1);
    cwpdb_store::test_support::summary_pages::rollback_journal(&path, RUNTIME).unwrap();
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let mut blocker =
        cwpdb_store::test_support::summary_pages::ExclusiveReadLock::acquire(&path, RUNTIME)
            .unwrap();
    let retained = Arc::new(Mutex::new(None));
    let retained_registration = Arc::clone(&retained);
    assert!(store
        .begin_operation_until_with(Instant::now() + TIMEOUT, move |cancellation| {
            *retained_registration.lock().unwrap() = Some(cancellation);
        })
        .is_err());
    blocker.release().unwrap();

    let operation = store.begin_operation(TIMEOUT).unwrap();
    retained
        .lock()
        .unwrap()
        .take()
        .expect("retain failed setup cancellation")
        .cancel();
    assert!(operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(1),
            ..Default::default()
        })
        .is_ok());
}

#[test]
fn stale_cancellation_cannot_interrupt_a_later_operation() {
    let (_temporary, path) = seeded_store(1);
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let first_operation = store.begin_operation(TIMEOUT).unwrap();
    let stale_cancellation = first_operation.cancellation();
    drop(first_operation);

    let second_operation = store.begin_operation(TIMEOUT).unwrap();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1));
        stale_cancellation.cancel();
    });
    let result = second_operation
        .finite_snapshot()
        .unwrap()
        .cancellation_probe(false);
    canceller.join().unwrap();
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn cancellation_and_deadlines_interrupt_in_flight_sql() {
    let (_temporary, path) = seeded_store(1);
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    let cancellation = operation.cancellation();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(5));
        cancellation.cancel();
    });
    assert!(long_query(&operation).is_err());
    canceller.join().unwrap();
    drop(operation);

    let operation = store.begin_operation(Duration::from_millis(5)).unwrap();
    assert!(long_query(&operation).is_err());
}

#[test]
fn deadline_applies_while_establishing_the_snapshot() {
    let (_temporary, path) = seeded_store(1);
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let expired = store.begin_operation(Duration::ZERO);
    assert!(expired.is_err());
    drop(expired);

    let operation = store.begin_operation(TIMEOUT).unwrap();
    assert_eq!(
        orders(
            &operation
                .root_conversations(ConversationSummaryWindow {
                    first: Some(1),
                    ..Default::default()
                })
                .unwrap()
        ),
        vec![0]
    );
}

#[test]
fn reads_are_bounded_and_use_the_root_keyset_index() {
    let (_temporary, path) = seeded_store(10_000);
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    let page = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(50),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.summaries.len(), 50);
    assert_eq!(page.summaries[0].key.creation_order, 9_999);
    assert_eq!(page.summaries[49].key.creation_order, 9_950);
}

#[test]
fn zero_pages_and_invalid_windows_are_explicit() {
    let (_temporary, path) = seeded_store(5);
    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    let empty = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(0),
            ..Default::default()
        })
        .unwrap();
    assert!(empty.summaries.is_empty());
    assert!(empty.has_next_page);
    assert!(!empty.has_previous_page);

    let combined_empty = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(1),
            last: Some(0),
            ..Default::default()
        })
        .unwrap();
    assert!(combined_empty.summaries.is_empty());
    assert!(combined_empty.has_next_page);
    assert!(combined_empty.has_previous_page);

    let first_and_last_zero = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(0),
            last: Some(0),
            ..Default::default()
        })
        .unwrap();
    assert!(first_and_last_zero.summaries.is_empty());
    assert!(first_and_last_zero.has_next_page);
    assert!(!first_and_last_zero.has_previous_page);

    let independently_sliced = operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(2),
            last: Some(3),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(orders(&independently_sliced), vec![4, 3]);
    assert!(independently_sliced.has_next_page);
    assert!(!independently_sliced.has_previous_page);
    assert!(operation
        .root_conversations(ConversationSummaryWindow {
            first: Some(MAX_CONVERSATION_SUMMARY_PAGE_SIZE + 1),
            ..Default::default()
        })
        .is_err());
    assert!(operation
        .root_conversations(ConversationSummaryWindow::default())
        .is_err());
    assert!(operation
        .root_conversations(ConversationSummaryWindow {
            after: Some(&ConversationSummaryKey {
                creation_order: 1,
                conversation_ref: "cwp://com.example.runtime/conversations/missing".into()
            }),
            first: Some(1),
            ..Default::default()
        })
        .is_err());
}

fn orders(page: &ConversationSummaryPage) -> Vec<i64> {
    page.summaries
        .iter()
        .map(|summary| summary.key.creation_order)
        .collect()
}

fn long_query(operation: &ConversationReadOperation<'_>) -> crate::CwrResult<i64> {
    operation.finite_snapshot()?.cancellation_probe(true)
}

#[test]
fn summary_membership_revision_ignores_content_but_tracks_archive_and_membership() {
    let (_temporary, path) = seeded_store(2);
    let read = || {
        let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
        let operation = store.begin_operation(TIMEOUT).unwrap();
        operation
            .root_conversations(ConversationSummaryWindow {
                first: Some(1),
                ..Default::default()
            })
            .unwrap()
            .membership_revision
    };
    let initial = read();
    cwpdb_store::test_support::summary_pages::update_content_without_membership(&path, RUNTIME)
        .unwrap();
    assert_eq!(read(), initial);
    cwpdb_store::test_support::summary_pages::archive_first(&path, RUNTIME).unwrap();
    let archived = read();
    assert_ne!(archived, initial);
    cwpdb_store::test_support::summary_pages::append(&path, RUNTIME, 2).unwrap();
    assert_ne!(read(), archived);
}
