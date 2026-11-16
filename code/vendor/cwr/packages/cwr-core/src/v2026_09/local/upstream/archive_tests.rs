#[test]
fn archive_waits_for_provider_stop_and_cancels_requests_arriving_during_the_wait() {
    let (_directory, runtime, _actor, _session, receiver) = fixture();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let admission = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "archive-active",
            person,
            None,
            "@tui new",
            Some("connection"),
        )
        .unwrap();
    let worker_runtime = runtime.clone();
    let worker_admission = admission.clone();
    let worker = thread::spawn(move || worker_runtime.execute_admission(&worker_admission, |_| {}));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::WorkRequested { .. }
    ));
    let local = runtime.admit_text(Some(admission.conversation_ref()), "@helper local work").unwrap();
    let local_control = ProcessAdapterStopControl::new();
    {
        let mut state = runtime.lock().unwrap();
        state
            .admission
            .claim_invocation(local.invocation_ref().unwrap(), current_unix_ms())
            .unwrap();
        state.active.insert(
            local.invocation_ref().unwrap().into(),
            local_control.clone(),
        );
    }
    let stop_runtime = runtime.clone();
    let conversation = admission.conversation_ref().to_owned();
    let stopping = thread::spawn(move || {
        stop_runtime.set_conversation_archived_with_stops_as(Some(&conversation), person, true)
    });
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::StopRequested { .. }
    ));
    assert!(local_control.is_requested(), "local cancellation must precede upstream acknowledgement");
    assert!(
        !runtime
            .conversation(admission.conversation_ref())
            .unwrap()
            .unwrap()
            .archived
    );
    let late = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "archive-late",
            person,
            Some(admission.conversation_ref()),
            "@tui queued",
            Some("connection"),
        )
        .unwrap();
    assert!(runtime.try_schedule_execution().unwrap().is_none());
    assert!(receiver.try_recv().is_err());
    runtime
        .acknowledge_upstream_stop("connection", admission.invocation_ref().unwrap())
        .unwrap();
    let (_, stopped) = stopping.join().unwrap().unwrap();
    assert!(stopped.contains(&admission.invocation_ref().unwrap().to_owned()));
    assert!(stopped.contains(&late.invocation_ref().unwrap().to_owned()));
    worker.join().unwrap().unwrap();
    assert_eq!(
        runtime
            .turn(&admission.record)
            .unwrap()
            .invocation
            .unwrap()
            .state,
        ActionInvocationState::Stopped
    );
    runtime
        .set_conversation_archived_as(Some(admission.conversation_ref()), person, false)
        .unwrap();
    assert!(runtime.try_schedule_execution().unwrap().is_none());
    assert_eq!(
        runtime
            .turn(&late.record)
            .unwrap()
            .invocation
            .unwrap()
            .attempt_count,
        0
    );
}

#[test]
fn canceling_archive_while_provider_stops_settles_cancellation_without_archiving() {
    let (_directory, runtime, _actor, _session, receiver) = fixture();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let archive = runtime.admit_text(None, "@helper archive").unwrap();
    let record = match runtime
        .lock()
        .unwrap()
        .admission
        .claim_invocation(archive.invocation_ref().unwrap(), current_unix_ms())
        .unwrap()
    {
        ConversationInvocationClaimOutcome::Claimed(claim) => claim.claim.invocation,
        _ => panic!("archive not runnable"),
    };
    let admission = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "provider-work",
            person,
            Some(archive.conversation_ref()),
            "@tui new",
            Some("connection"),
        )
        .unwrap();
    let worker_runtime = runtime.clone();
    let worker_admission = admission.clone();
    let worker = thread::spawn(move || worker_runtime.execute_admission(&worker_admission, |_| {}));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::WorkRequested { .. }
    ));
    // Hold the second invocation between its claim and provider registration.
    let claimed_admission = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "claimed-before-archive",
            person,
            Some(archive.conversation_ref()),
            "@tui pending registration",
            Some("connection"),
        )
        .unwrap();
    let (claimed_record, claimed_route) = {
        let mut state = runtime.lock().unwrap();
        let record = match state
            .admission
            .claim_invocation(
                claimed_admission.invocation_ref().unwrap(),
                current_unix_ms(),
            )
            .unwrap()
        {
            ConversationInvocationClaimOutcome::Claimed(claim) => claim.claim.invocation,
            _ => panic!("provider invocation not runnable"),
        };
        let route = retained_upstream_route(&state, &record.invocation_ref).unwrap();
        (record, route)
    };
    let (_, mutation) = runtime
        .prepare_conversation_archive_as(Some(archive.conversation_ref()), person, true)
        .unwrap();
    let stop_runtime = runtime.clone();
    let archive_record = record.clone();
    let stopping = thread::spawn(move || {
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        let mut events = Vec::new();
        stop_runtime
            .publish_invocation_terminal(
                &archive_record,
                InvocationTerminal {
                    status: ActionResponseStatus::COMPLETED,
                    summary: "Archived".into(),
                    result: Some(json!({"text":"Archived"})),
                    error: None,
                },
                "Archived".into(),
                Some(ConversationIntrinsicMutation::Archive(mutation)),
                &queue,
                &mut |event| events.push(event),
            )
            .unwrap();
        events
    });
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::StopRequested { .. }
    ));
    runtime
        .execute_upstream_work(
            &claimed_record,
            claimed_route,
            &Rc::new(RefCell::new(VecDeque::new())),
            &mut |_| {},
        )
        .unwrap();
    assert!(!runtime
        .lock()
        .unwrap()
        .upstream_work
        .contains_key(&claimed_record.invocation_ref));
    assert!(receiver.try_recv().is_err());
    runtime
        .stop_invocation(&record.invocation_ref, None)
        .unwrap();
    let fresh = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "fresh-during-canceled-archive",
            person,
            Some(archive.conversation_ref()),
            "@tui fresh",
            Some("connection"),
        )
        .unwrap();
    runtime
        .acknowledge_upstream_stop("connection", admission.invocation_ref().unwrap())
        .unwrap();
    let events = stopping.join().unwrap();
    runtime
        .settle_stopped_execution(&record.invocation_ref)
        .unwrap();
    // Settlement is published independently by the provider acknowledgment;
    // this canceled archive publisher need not emit the provider's terminal.
    let _ = events;
    worker.join().unwrap().unwrap();
    assert_eq!(runtime.turn(&admission.record).unwrap().invocation.unwrap().state, ActionInvocationState::Stopped);
    assert!(
        !runtime
            .conversation(archive.conversation_ref())
            .unwrap()
            .unwrap()
            .archived
    );
    assert_eq!(
        runtime
            .turn(&fresh.record)
            .unwrap()
            .invocation
            .unwrap()
            .state,
        ActionInvocationState::Admitted
    );
    assert_eq!(
        runtime
            .turn(&archive.record)
            .unwrap()
            .invocation
            .unwrap()
            .state,
        ActionInvocationState::Stopped
    );
    assert_eq!(
        runtime
            .turn(&claimed_admission.record)
            .unwrap()
            .invocation
            .unwrap()
            .state,
        ActionInvocationState::Stopped
    );
    assert!(runtime.lock().unwrap().archiving_conversations.is_empty());
}

#[test]
fn archive_requests_all_provider_stops_before_waiting_for_any_acknowledgement() {
    let (_directory, runtime, _actor, _session, receiver) = fixture();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let first = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "first-active",
            person,
            None,
            "@tui first",
            Some("connection"),
        )
        .unwrap();
    let first_runtime = runtime.clone();
    let first_work = first.clone();
    let first_worker = thread::spawn(move || first_runtime.execute_admission(&first_work, |_| {}));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::WorkRequested { .. }
    ));
    let second = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "second-active",
            person,
            Some(first.conversation_ref()),
            "@tui second",
            Some("connection"),
        )
        .unwrap();
    // Claim a second provider independently to exercise multiple simultaneous
    // upstream executions, as happens with independent delegated groups.
    let (record, route) = {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claim) = state.admission
            .claim_invocation(second.invocation_ref().unwrap(), current_unix_ms()).unwrap() else { panic!("claim failed") };
        let route = retained_upstream_route(&state, second.invocation_ref().unwrap()).unwrap();
        (claim.claim.invocation, route)
    };
    let second_runtime = runtime.clone();
    let second_worker = thread::spawn(move || {
        second_runtime.execute_upstream_work(
            &record,
            route,
            &Rc::new(RefCell::new(VecDeque::new())),
            &mut |_| {},
        )
    });
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::WorkRequested { .. }
    ));
    let archive_runtime = runtime.clone();
    let conversation = first.conversation_ref().to_owned();
    let archiving = thread::spawn(move || {
        archive_runtime.set_conversation_archived_with_stops_as(Some(&conversation), person, true)
    });
    let mut requested = BTreeSet::new();
    for _ in 0..2 {
        let UpstreamEvent::StopRequested { invocation_ref } = receiver.recv_timeout(Duration::from_secs(2)).unwrap() else { panic!("missing stop request") };
        requested.insert(invocation_ref);
    }
    assert_eq!(requested, [first.invocation_ref().unwrap().to_owned(), second.invocation_ref().unwrap().to_owned()].into());
    for reference in &requested { runtime.acknowledge_upstream_stop("connection", reference).unwrap(); }
    let (_, stopped) = archiving.join().unwrap().unwrap();
    assert_eq!(stopped.into_iter().collect::<BTreeSet<_>>(), requested);
    first_worker.join().unwrap().unwrap();
    second_worker.join().unwrap().unwrap();
}

include!("archive_scheduling_tests.rs");
