#[test]
fn schedules_recent_and_archive_reads_survive_more_than_twenty_thousand_occurrences() {
    let (_temporary, runtime, _definition, owner, _) = installed_schedule_fixture();
    let person = participation_ref(&runtime, "person");
    let schedule = runtime
        .schedules_module_as(&owner, &person)
        .unwrap()
        .list()
        .unwrap()
        .pop()
        .unwrap();
    let mut store = {
        let state = runtime.lock().unwrap();
        schedule_store(&state).unwrap()
    };
    let newest = store
        .seed_and_verify_history_fixture(&schedule, 20_001)
        .unwrap();

    let recent = runtime
        .schedules_module_as(&owner, &person)
        .unwrap()
        .recent_runs(&schedule.schedule_ref, 1)
        .unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].occurrence_ref, newest);
    assert!(runtime
        .scan_schedule_archive_candidates()
        .unwrap()
        .iter()
        .any(|(_, run)| run.occurrence_ref == newest));
}
