use super::*;
#[test]
fn concurrent_event_retries_cannot_ingest_the_same_assignment_and_guards_release_on_error() {
    let active = std::sync::Arc::new(Mutex::new(std::collections::BTreeSet::new()));
    let first = EventPoll::reserve(&active, "first").unwrap().unwrap();
    let other = active.clone();
    std::thread::spawn(move || {
        assert!(EventPoll::reserve(&other, "first").unwrap().is_none());
        let _independent = EventPoll::reserve(&other, "second").unwrap().unwrap();
    })
    .join()
    .unwrap();
    assert_eq!(active.lock().unwrap().len(), 1);
    drop(first);
    let failed: CwrResult<()> = (|| {
        let _guard = EventPoll::reserve(&active, "first")?.unwrap();
        Err("fixture interruption".into())
    })();
    assert!(failed.is_err());
    assert!(active.lock().unwrap().is_empty());
    assert!(EventPoll::reserve(&active, "first").unwrap().is_some());
}
