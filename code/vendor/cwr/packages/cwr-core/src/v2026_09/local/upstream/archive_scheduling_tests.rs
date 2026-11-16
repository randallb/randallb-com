#[test]
fn scheduled_archive_wait_serializes_restore_and_keeps_stop_actions_schedulable() {
    for cancel_archive in [false, true] {
        let (_directory, runtime, _actor, _session, receiver) = fixture();
        let person = crate::v2026_09::test_fixture::actor_ref("person");
        let work = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                "scheduled-provider",
                person,
                None,
                "@tui work",
                Some("connection"),
            )
            .unwrap();
        let worker_runtime = runtime.clone();
        let worker_work = work.clone();
        let worker = thread::spawn(move || worker_runtime.execute_admission(&worker_work, |_| {}));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            UpstreamEvent::WorkRequested { .. }
        ));
        let archive = runtime
            .admit_action(
                Some(work.conversation_ref()),
                "@cwr--archive--conversation",
                json!({}),
            )
            .unwrap();
        let mut archive_task = runtime
            .try_schedule_execution()
            .unwrap()
            .expect("archive task");
        assert_eq!(
            archive_task.record().invocation_ref,
            archive.invocation_ref().unwrap()
        );
        let archiving = thread::spawn(move || {
            archive_task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
        });
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            UpstreamEvent::StopRequested { .. }
        ));
        let restore = runtime
            .admit_action(
                Some(work.conversation_ref()),
                "@cwr--restore--conversation",
                json!({}),
            )
            .unwrap();
        assert!(runtime.try_schedule_execution().unwrap().is_none());
        if cancel_archive {
            let stop = runtime
                .admit_action(
                    Some(work.conversation_ref()),
                    "@cwr--stop--invocation",
                    json!({"invocation_ref":archive.invocation_ref().unwrap()}),
                )
                .unwrap();
            let mut stop_task = runtime
                .try_schedule_execution()
                .unwrap()
                .expect("stop remains schedulable");
            assert_eq!(
                stop_task.record().invocation_ref,
                stop.invocation_ref().unwrap()
            );
            stop_task
                .execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
                .unwrap();
            assert!(runtime
                .turn(&stop.record)
                .unwrap()
                .invocation
                .unwrap()
                .state
                .is_terminal());
            assert!(matches!(
                runtime.turn(&archive.record).unwrap().invocation.unwrap().state,
                ActionInvocationState::Stopping | ActionInvocationState::Stopped
            ));
            // The archive executor may already have returned; the provider has not.
            assert_ne!(runtime.turn(&work.record).unwrap().invocation.unwrap().state,
                ActionInvocationState::Stopped);
        }
        runtime
            .acknowledge_upstream_stop("connection", work.invocation_ref().unwrap())
            .unwrap();
        archiving.join().unwrap().unwrap();
        worker.join().unwrap().unwrap();
        assert_eq!(
            runtime
                .conversation(work.conversation_ref())
                .unwrap()
                .unwrap()
                .archived,
            !cancel_archive
        );
        let mut restore_task = runtime
            .try_schedule_execution()
            .unwrap()
            .expect("restore after archive settles");
        assert_eq!(
            restore_task.record().invocation_ref,
            restore.invocation_ref().unwrap()
        );
        restore_task
            .execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
            .unwrap();
        assert!(
            !runtime
                .conversation(work.conversation_ref())
                .unwrap()
                .unwrap()
                .archived
        );
    }
}

#[test]
fn scheduled_archive_orders_host_restore_after_provider_acknowledgement() {
    let (_directory, runtime, _actor, _session, receiver) = fixture();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let work = runtime.submit_text_in_upstream_scope(
        "cwp://example.test/ingress", "mixed-control-provider", person, None,
        "@tui work", Some("connection"),
    ).unwrap();
    let worker_runtime = runtime.clone();
    let worker_work = work.clone();
    let worker = thread::spawn(move || worker_runtime.execute_admission(&worker_work, |_| {}));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::WorkRequested { .. }
    ));
    runtime
        .admit_action(
            Some(work.conversation_ref()),
            "@cwr--archive--conversation",
            json!({}),
        )
        .unwrap();
    let mut archive = runtime
        .try_schedule_execution()
        .unwrap()
        .expect("archive task");
    let archiving = thread::spawn(move || {
        archive.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
    });
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::StopRequested { .. }
    ));
    assert!(runtime
        .archive_mutations
        .active_conversation_refs
        .lock()
        .unwrap()
        .contains(work.conversation_ref()));
    let restore_runtime = runtime.clone();
    let conversation = work.conversation_ref().to_owned();
    let (sender, restored) = std::sync::mpsc::channel();
    let restoring = thread::spawn(move || {
        let result = restore_runtime.set_conversation_archived_with_stops_as(
            Some(&conversation),
            person,
            false,
        );
        sender.send(result).unwrap();
    });
    assert!(matches!(restored.recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)));
    runtime.acknowledge_upstream_stop("connection", work.invocation_ref().unwrap()).unwrap();
    archiving.join().unwrap().unwrap();
    restored.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    restoring.join().unwrap();
    worker.join().unwrap().unwrap();
    assert!(!runtime.conversation(work.conversation_ref()).unwrap().unwrap().archived);
}

#[test]
fn stopping_a_scheduled_archive_releases_fences_without_provider_acknowledgement() {
    let (_directory, runtime, _actor, _session, receiver) = fixture();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let work = runtime.submit_text_in_upstream_scope(
        "cwp://example.test/ingress", "never-acknowledged", person, None,
        "@tui work", Some("connection"),
    ).unwrap();
    let worker_runtime = runtime.clone();
    let worker_work = work.clone();
    let worker = thread::spawn(move || worker_runtime.execute_admission(&worker_work, |_| {}));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::WorkRequested { .. }
    ));
    let archive = runtime
        .admit_action(
            Some(work.conversation_ref()),
            "@cwr--archive--conversation",
            json!({}),
        )
        .unwrap();
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    let (sender, done) = std::sync::mpsc::channel();
    let archiving = thread::spawn(move || {
        sender.send(task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})).unwrap();
    });
    assert!(matches!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(), UpstreamEvent::StopRequested { .. }));
    // A waiting archive excludes only its own conversation from the archive lane.
    let unrelated = runtime
        .admit_action(None, "@cwr--restore--conversation", json!({}))
        .unwrap();
    let mut unrelated_task = runtime
        .try_schedule_execution()
        .unwrap()
        .expect("unrelated control remains schedulable");
    assert_eq!(
        unrelated_task.record().invocation_ref,
        unrelated.invocation_ref().unwrap()
    );
    unrelated_task
        .execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
        .unwrap();
    runtime
        .stop_intrinsic_invocation_as(
            Some(work.conversation_ref()),
            archive.invocation_ref(),
            person,
            None,
        )
        .unwrap();
    done.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    archiving.join().unwrap();
    worker.join().unwrap().unwrap();
    assert!(!runtime.conversation(work.conversation_ref()).unwrap().unwrap().archived);
    assert!(runtime.lock().unwrap().archiving_conversations.is_empty());
    let restore = runtime
        .admit_action(
            Some(work.conversation_ref()),
            "@cwr--restore--conversation",
            json!({}),
        )
        .unwrap();
    let mut restore_task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(restore_task.record().invocation_ref, restore.invocation_ref().unwrap());
    restore_task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {}).unwrap();
    let fresh = runtime.admit_text(Some(work.conversation_ref()), "@helper fresh").unwrap();
    assert!(require_no_stop_fence(&runtime.lock().unwrap(), fresh.invocation_ref().unwrap()).is_ok());
}

#[test]
fn archive_revalidates_actor_after_provider_acknowledgement() {
    for scheduled in [false, true] {
        let (_directory, runtime, _actor, _session, receiver) = fixture();
        let person = crate::v2026_09::test_fixture::actor_ref("person");
        let work = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                "archive-removed-actor",
                person,
                None,
                "@tui work",
                Some("connection"),
            )
            .unwrap();
        let work_runtime = runtime.clone();
        let work_admission = work.clone();
        let work_thread = thread::spawn(move || work_runtime.execute_admission(&work_admission, |_| {}));
        assert!(matches!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(), UpstreamEvent::WorkRequested { .. }));
        let conversation = work.conversation_ref().to_owned();
        let archive_runtime = runtime.clone();
        let archiving = if scheduled {
            let archive = runtime
                .admit_action(
                    Some(&conversation),
                    "@cwr--archive--conversation",
                    json!({}),
                )
                .unwrap();
            let mut task = runtime.try_schedule_execution().unwrap().unwrap();
            assert_eq!(
                task.record().invocation_ref,
                archive.invocation_ref().unwrap()
            );
            thread::spawn(move || {
                task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
            })
        } else {
            thread::spawn(move || {
                archive_runtime
                    .set_conversation_archived_as(Some(&conversation), person, true)
                    .map(|_| ())
            })
        };
        assert!(matches!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(), UpstreamEvent::StopRequested { .. }));
        runtime.lock().unwrap().graph.actors.retain(|actor| actor.actor_ref != person);
        runtime.acknowledge_upstream_stop("connection", work.invocation_ref().unwrap()).unwrap();
        assert!(archiving.join().unwrap().is_err());
        assert_eq!(work_thread.join().unwrap().unwrap().invocation.unwrap().state, ActionInvocationState::Stopped);
        assert!(!runtime.conversation(work.conversation_ref()).unwrap().unwrap().archived);
    }
}
