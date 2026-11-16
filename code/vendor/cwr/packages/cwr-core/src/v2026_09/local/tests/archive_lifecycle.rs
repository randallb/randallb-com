#[test]
fn archive_retry_retains_order_after_a_failed_terminal_write() {
    let (temporary, runtime) = participation_runtime();
    let active = claimed_participation_parent(&runtime);
    let control = ProcessAdapterStopControl::new();
    runtime
        .lock()
        .unwrap()
        .active
        .insert(active.invocation_ref.clone(), control.clone());
    let archive = runtime
        .admit_action(
            active.conversation_ref.as_deref(),
            "@cwr--archive--conversation",
            json!({}),
        )
        .unwrap();
    let database_path = temporary.path().join("runtime.sqlite");
    cwpdb_store::test_support::completion::install_write_failure(&database_path).unwrap();
    let mut first = runtime.try_schedule_execution().unwrap().unwrap();
    assert!(first
        .execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
        .is_err());
    drop(first);
    let record = runtime.turn(&archive.record).unwrap().invocation.unwrap();
    assert_eq!(record.state, ActionInvocationState::Running);
    assert!(!record.attempt_active);
    assert!(control.is_requested());
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(&active.invocation_ref)
            .unwrap()
            .state,
        ActionInvocationState::Stopping
    );
    assert!(
        !runtime
            .conversation(archive.conversation_ref())
            .unwrap()
            .unwrap()
            .archived
    );
    cwpdb_store::test_support::completion::remove_write_failure(&database_path).unwrap();
    // Confirm the simulated executor exited before retrying the archive.
    runtime
        .settle_stopped_execution(&active.invocation_ref)
        .unwrap();
    let mut retry = runtime.try_schedule_execution().unwrap().unwrap();
    let (sender, done) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        sender
            .send(retry.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {}))
            .unwrap()
    });
    done.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    worker.join().unwrap();
    assert!(
        runtime
            .conversation(archive.conversation_ref())
            .unwrap()
            .unwrap()
            .archived
    );
}

#[test]
fn archive_terminal_observer_can_synchronously_restore_the_same_conversation() {
    let (_temporary, runtime) = participation_runtime();
    let archive = runtime
        .admit_action(None, "@cwr--archive--conversation", json!({}))
        .unwrap();
    let conversation = archive.conversation_ref().to_owned();
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    let observer_runtime = runtime.clone();
    let observer_conversation = conversation.clone();
    let (sender, done) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        sender
            .send(
                task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |event| {
                    if matches!(event, LocalRuntimeEvent::Terminal { .. }) {
                        observer_runtime
                            .set_conversation_archived_as(
                                Some(&observer_conversation),
                                &person,
                                false,
                            )
                            .unwrap();
                    }
                }),
            )
            .unwrap()
    });
    done.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    worker.join().unwrap();
    assert!(
        !runtime
            .conversation(&conversation)
            .unwrap()
            .unwrap()
            .archived
    );
}

#[test]
fn archive_revalidates_actor_after_waiting_for_control_order() {
    let (_temporary, runtime) = participation_runtime();
    let work = claimed_participation_parent(&runtime);
    let conversation = work.conversation_ref.clone().unwrap();
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let first = runtime.reserve_conversation_archive(&conversation).unwrap();
    let archive_runtime = runtime.clone();
    let archive_conversation = conversation.clone();
    let caller = person.clone();
    let (sender, done) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        sender
            .send(archive_runtime.set_conversation_archived_as(
                Some(&archive_conversation),
                &caller,
                true,
            ))
            .unwrap()
    });
    assert!(matches!(
        done.recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    runtime
        .lock()
        .unwrap()
        .graph
        .actors
        .retain(|actor| actor.actor_ref != person);
    drop(first);
    assert!(done.recv_timeout(Duration::from_secs(2)).unwrap().is_err());
    worker.join().unwrap();
    assert!(
        !runtime
            .conversation(&conversation)
            .unwrap()
            .unwrap()
            .archived
    );
}
