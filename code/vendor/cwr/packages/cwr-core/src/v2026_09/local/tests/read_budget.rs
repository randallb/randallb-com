#[test]
fn read_budget_bounds_runtime_lock_waits_for_catalog_metadata_and_pages() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(fixture_workspace(), directory.path().join("state.sqlite3"), fixture_host_identity()).unwrap();
    let actor = runtime.catalog().unwrap().acting_actor.actor_ref;
    let _held = runtime.lock().unwrap();
    let started = Instant::now();
    assert!(runtime.upstream_catalog_as_until(&actor, None, Instant::now()+Duration::from_millis(20), ||false).is_err());
    assert!(runtime.host_conversation_metadata_until("cwp://example.test/missing", Instant::now()+Duration::from_millis(20), ||false).is_err());
    assert!(runtime.participant_root_conversation_summaries_cancellable(&actor, &ConversationSummaryPageRequest::default(), None, Instant::now()+Duration::from_millis(20), ||false, |_|{}).is_err());
    assert!(runtime.participant_conversation_timeline_cancellable("cwp://example.test/missing", &actor, &ConversationRecordPageRequest::default(), Instant::now()+Duration::from_millis(20), ||false, |_|{}).is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    let started = Instant::now();
    assert!(runtime.upstream_catalog_as_until(&actor,None,Instant::now()+Duration::from_secs(10),||true).is_err());
    assert!(runtime.host_conversation_metadata_until("cwp://example.test/missing",Instant::now()+Duration::from_secs(10),||true).is_err());
    assert!(started.elapsed() < Duration::from_millis(100));
}
