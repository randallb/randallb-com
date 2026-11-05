#[test]
fn cloud_delivery_rechecks_pending_stop_fence_and_allows_successful_replay() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("delivery.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "@helper delivery fence").unwrap();
    let reference = admission.invocation_ref().unwrap();
    runtime
        .lock()
        .unwrap()
        .admission
        .claim_invocation(reference, 10)
        .unwrap();
    assert!(runtime.cloud_execution_delivery_record(reference).is_ok());
    // Model the accepted in-memory stop fence before the retained state update.
    runtime
        .lock()
        .unwrap()
        .stopping_roots
        .insert(reference.into(), 1);
    assert!(
        runtime.cloud_execution_record(reference, false).is_ok(),
        "cleanup remains authorized"
    );
    assert!(runtime
        .cloud_execution_delivery_record(reference)
        .unwrap_err()
        .to_string()
        .contains("stopping"));
    runtime.lock().unwrap().stopping_roots.remove(reference);
    runtime
        .lock()
        .unwrap()
        .admission
        .finish_invocation(
            reference,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Done".into(),
                result: Some(json!({})),
                error: None,
            },
            20,
        )
        .unwrap();
    assert!(runtime.cloud_execution_record(reference, true).is_err());
    assert_eq!(
        runtime
            .cloud_execution_delivery_record(reference)
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );
    // A newly accepted ancestor/root stop also fences already-retained output.
    runtime
        .lock()
        .unwrap()
        .stopping_roots
        .insert(reference.into(), 1);
    assert!(runtime.cloud_execution_delivery_record(reference).is_err());
}
#[test]
fn cloud_delivery_refuses_stopped_work_while_cleanup_can_inspect_it() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("stopped-delivery.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime
        .admit_text(None, "@helper stopped delivery")
        .unwrap();
    let reference = admission.invocation_ref().unwrap();
    {
        let mut state = runtime.lock().unwrap();
        state.admission.claim_invocation(reference, 10).unwrap();
        state
            .active
            .insert(reference.into(), ProcessAdapterStopControl::default());
    }
    runtime.stop_invocation(reference, None).unwrap();
    assert!(runtime.cloud_execution_record(reference, false).is_ok());
    assert!(runtime.cloud_execution_delivery_record(reference).is_err());
}
