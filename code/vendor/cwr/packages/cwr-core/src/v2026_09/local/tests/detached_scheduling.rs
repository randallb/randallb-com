#[test]
fn direct_execution_drains_its_async_own_conversation_descendant() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    for binding in &mut runtime.lock().unwrap().graph.actor_integrations {
        binding.adapter_ref = "cwp://example.test/adapters/unavailable".into();
    }
    let source = runtime.admit_text(None, "@helper delegate").unwrap();
    let child_ref = stage_async_own_conversation_child(&runtime, &source);

    runtime.execute_admission(&source, |_| {}).unwrap();

    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(&child_ref)
            .unwrap()
            .state,
        ActionInvocationState::Failed
    );
}

#[test]
fn direct_descendant_drain_stops_when_shutdown_preserves_work() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let source = runtime.admit_text(None, "@helper delegate").unwrap();
    let child_ref = stage_async_own_conversation_child(&runtime, &source);
    runtime.lock().unwrap().shutting_down = true;

    runtime
        .execute_independent_descendants(
            source.invocation_ref().unwrap(),
            &AdapterModelOutputMailbox::discard(),
            &mut |_| {},
        )
        .unwrap();

    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(&child_ref)
            .unwrap()
            .state,
        ActionInvocationState::Admitted
    );
}
