#[test]
fn default_timeline_activation_can_be_disabled_without_stranding_history() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("timeline-rollout.sqlite");
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime
        .admit_text(None, "@helper managed timeline")
        .unwrap();
    let conversation_ref = admission.conversation_ref().to_owned();
    assert!(runtime
        .lock()
        .unwrap()
        .admission
        .retained_timeline(&conversation_ref)
        .is_some());
    drop(runtime);

    let reopened = LocalRuntime::open_with_state_path_and_options(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
        LocalRuntimeOptions {
            conversation_timeline_rollout: ConversationTimelineRollout::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(reopened
        .lock()
        .unwrap()
        .admission
        .retained_timeline(&conversation_ref)
        .is_some());
    let fresh = reopened
        .admit_text(None, "@helper legacy while disabled")
        .unwrap();
    assert!(reopened
        .lock()
        .unwrap()
        .admission
        .retained_timeline(fresh.conversation_ref())
        .is_none());
    drop(reopened);

    let enabled = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        &state_path,
        fixture_host_identity(),
    )
    .unwrap();
    // Re-enabling only activates new conversations; disabled history stays legacy.
    assert!(enabled
        .lock()
        .unwrap()
        .admission
        .retained_timeline(fresh.conversation_ref())
        .is_none());
    let fresh = enabled.admit_text(None, "@helper canonical again").unwrap();
    assert!(enabled
        .lock()
        .unwrap()
        .admission
        .retained_timeline(fresh.conversation_ref())
        .is_some());
}

#[test]
fn normal_startup_backfills_quiescent_legacy_history_without_configuration() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("legacy-startup.sqlite");
    let runtime = LocalRuntime::open_with_state_path_and_options(
        fixture_workspace(),
        &path,
        fixture_host_identity(),
        LocalRuntimeOptions {
            conversation_timeline_rollout: ConversationTimelineRollout::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let exchange = runtime
        .lock()
        .unwrap()
        .admission
        .record_host_exchange(
            None,
            person,
            helper,
            "Old request".into(),
            "Old response".into(),
        )
        .unwrap();
    let conversation_ref = exchange.conversation.conversation_ref;
    assert!(runtime
        .lock()
        .unwrap()
        .admission
        .retained_timeline(&conversation_ref)
        .is_none());
    drop(runtime);

    let reopened =
        LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity())
            .unwrap();
    let state = reopened.lock().unwrap();
    let timeline = state
        .admission
        .retained_timeline(&conversation_ref)
        .unwrap();
    let items = &timeline
        .connection(&timeline.root_connection_ref)
        .unwrap()
        .items;
    assert_eq!(
        items
            .iter()
            .map(|item| item.content.reference())
            .collect::<Vec<_>>(),
        [exchange.request_message_ref, exchange.response_message_ref]
    );
}
