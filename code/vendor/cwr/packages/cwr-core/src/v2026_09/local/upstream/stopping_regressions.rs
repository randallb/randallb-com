#[test]
fn stopping_upstream_fences_followups_until_acknowledgement() {
    let (_directory, runtime, _actor, _session, receiver) = fixture();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let first = runtime.submit_text_in_upstream_scope("cwp://example.test/ingress", "first-stop",
        person, None, "@tui first", Some("connection")).unwrap();
    // Select both requests into one task before the first is canceled.
    let next = runtime.submit_text_in_upstream_scope("cwp://example.test/ingress", "next-stop",
        person, Some(first.conversation_ref()), "@tui next", Some("connection")).unwrap();
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    let worker = thread::spawn(move || task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {}));
    assert!(matches!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(), UpstreamEvent::WorkRequested { .. }));
    runtime.stop_invocation(first.invocation_ref().unwrap(), None).unwrap();
    assert!(matches!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(), UpstreamEvent::StopRequested { .. }));
    worker.join().unwrap().unwrap();
    assert_eq!(runtime.turn(&next.record).unwrap().invocation.unwrap().state, ActionInvocationState::Admitted);
    assert!(runtime.try_schedule_execution().unwrap().is_none());
    assert!(runtime.claim_execution(next.invocation_ref().unwrap(), &ProcessAdapterStopControl::new()).unwrap().is_none());
    assert!(receiver.try_recv().is_err());
    runtime.acknowledge_upstream_stop("connection", first.invocation_ref().unwrap()).unwrap();
    assert!(runtime.try_schedule_execution().unwrap().is_some());
}

#[test]
fn archive_waits_for_a_previously_requested_upstream_stop() {
    for scheduled in [false, true] {
        let (_directory, runtime, _actor, _session, receiver) = fixture();
        let person = crate::v2026_09::test_fixture::actor_ref("person");
        let first = runtime.submit_text_in_upstream_scope("cwp://example.test/ingress", "archive-stop",
            person, None, "@tui first", Some("connection")).unwrap();
        let worker_runtime = runtime.clone();
        let admission = first.clone();
        let worker = thread::spawn(move || worker_runtime.execute_admission(&admission, |_| {}));
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        runtime.stop_invocation(first.invocation_ref().unwrap(), None).unwrap();
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap().unwrap();
        let archive_runtime = runtime.clone();
        let conversation = first.conversation_ref().to_owned();
        let archive = thread::spawn(move || {
            if scheduled {
                let admission = archive_runtime.admit_action(Some(&conversation), "@cwr--archive--conversation", json!({})).unwrap();
                archive_runtime.execute_admission(&admission, |_| {}).map(|_| ())
            } else {
                archive_runtime.set_conversation_archived_with_stops_as(Some(&conversation), person, true).map(|_| ())
            }
        });
        // Receiving a new stop proves archive target selection reached the provider.
        assert!(matches!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(), UpstreamEvent::StopRequested { .. }));
        assert!(!archive.is_finished());
        assert!(!runtime.conversation(first.conversation_ref()).unwrap().unwrap().archived);
        runtime.acknowledge_upstream_stop("connection", first.invocation_ref().unwrap()).unwrap();
        archive.join().unwrap().unwrap();
        assert!(runtime.conversation(first.conversation_ref()).unwrap().unwrap().archived);
    }
}
