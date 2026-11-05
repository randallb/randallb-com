#[test]
fn deadline_stop_retries_a_full_queue_without_waiting_for_acknowledgement() {
    let (_directory, runtime, _actor, session, receiver) = fixture();
    let admission = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "deadline",
            crate::v2026_09::test_fixture::actor_ref("person"),
            None,
            "@tui new",
            Some("connection"),
        )
        .unwrap();
    let worker_runtime = runtime.clone();
    let worker_admission = admission.clone();
    let worker = thread::spawn(move || worker_runtime.execute_admission(&worker_admission, |_| {}));
    receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    let invocation = admission.invocation_ref().unwrap().to_owned();
    let (deadline_stops, blocked_deadline_stops) = std::sync::mpsc::sync_channel(0);
    let mut state = runtime.lock().unwrap();
    let provider = state.upstream_providers.get_mut(&session).unwrap();
    provider.deadline_stop_sender = deadline_stops;
    drop(state);

    let started = Instant::now();
    let outcome = runtime
        .stop_invocation_as_without_upstream_wait(
            &invocation,
            crate::v2026_09::test_fixture::actor_ref("person"),
            Some("Deadline expired".into()),
        )
        .unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(matches!(outcome, LocalStopOutcome::Accepted { .. }));
    assert!(matches!(
        blocked_deadline_stops.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::StopRequested { invocation_ref } if invocation_ref == invocation
    ));
    assert_eq!(
        worker.join().unwrap().unwrap().invocation.unwrap().state,
        ActionInvocationState::Stopping
    );
    assert!(runtime
        .lock()
        .unwrap()
        .upstream_work
        .contains_key(&invocation));
    runtime
        .acknowledge_upstream_stop("connection", &invocation)
        .unwrap();
    assert_eq!(
        runtime
            .turn(&admission.record)
            .unwrap()
            .invocation
            .unwrap()
            .state,
        ActionInvocationState::Stopped
    );
    assert!(!runtime
        .lock()
        .unwrap()
        .upstream_work
        .contains_key(&invocation));
    let runtime_state = Arc::downgrade(&runtime.inner);
    drop(runtime);
    assert!(
        runtime_state.upgrade().is_none(),
        "best-effort stop delivery must not retain the runtime"
    );
}
