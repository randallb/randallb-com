#[test]
fn archive_does_not_republish_work_stopped_before_its_transaction() {
    let (_temporary, runtime) = participation_runtime();
    let historical = claimed_participation_parent(&runtime);
    let conversation = historical.conversation_ref.as_deref().unwrap();
    runtime.stop_invocation(&historical.invocation_ref, None).unwrap();
    runtime.settle_stopped_execution(&historical.invocation_ref).unwrap();
    let queued = runtime.admit_text(Some(conversation), "@helper queued").unwrap();
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let (_, stopped) = runtime
        .set_conversation_archived_with_stops_as(Some(conversation), &person, true)
        .unwrap();
    assert_eq!(stopped, vec![queued.invocation_ref().unwrap().to_owned()]);
    let (_, stopped_again) = runtime
        .set_conversation_archived_with_stops_as(Some(conversation), &person, true)
        .unwrap();
    assert!(stopped_again.is_empty());
}

#[test]
fn archive_of_an_own_scope_execution_stops_its_owner_without_stopping_outer_work() {
    let (_temporary, runtime) = participation_runtime();
    let outer = claimed_participation_parent(&runtime);
    let owner_ref = archive_test_child(&runtime, &outer, "own-scope-owner");
    let owner = {
        let mut state = runtime.lock().unwrap();
        match state.admission.claim_invocation(&owner_ref, current_unix_ms()).unwrap() {
            ConversationInvocationClaimOutcome::Claimed(claim) => claim.claim.invocation,
            _ => panic!("owner not runnable"),
        }
    };
    let own_conversation = runtime.lock().unwrap().admission
        .execution_conversation_ref(&owner_ref).unwrap().to_owned();
    assert_ne!(owner.conversation_ref.as_deref(), Some(own_conversation.as_str()));
    let child = archive_test_child(&runtime, &owner, "inside-own-scope");
    let (_, stopped) = runtime.set_conversation_archived_with_stops_as(
        Some(&own_conversation), &owner.target_actor_ref, true,
    ).unwrap();
    assert!(stopped.contains(&owner_ref));
    assert!(stopped.contains(&child));
    assert!(!stopped.contains(&outer.invocation_ref));
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(&owner_ref)
            .unwrap()
            .state,
        ActionInvocationState::Stopping
    );
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(&outer.invocation_ref)
            .unwrap()
            .state,
        ActionInvocationState::Running
    );
}

#[test]
fn archive_canceled_while_waiting_for_mutation_guard_leaves_new_work_untouched() {
    let (_temporary, runtime) = participation_runtime();
    let archive = runtime.admit_action(None, "@cwr--archive--conversation", json!({})).unwrap();
    let record = {
        let mut state = runtime.lock().unwrap();
        match state.admission.claim_invocation(archive.invocation_ref().unwrap(), current_unix_ms()).unwrap() {
            ConversationInvocationClaimOutcome::Claimed(claim) => claim.claim.invocation,
            _ => panic!("archive not runnable"),
        }
    };
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let (_, mutation) = runtime.prepare_conversation_archive_as(
        Some(archive.conversation_ref()), &person, true).unwrap();
    let held = runtime.archive_mutations.acquire(archive.conversation_ref(), || Ok(true)).unwrap();
    let waiting_runtime = runtime.clone();
    let archive_record = record.clone();
    let (sender, done) = std::sync::mpsc::channel();
    let waiting = std::thread::spawn(move || {
        let result = waiting_runtime.publish_invocation_terminal(
            &archive_record,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Archived".into(),
                result: Some(json!({"text":"Archived"})),
                error: None,
            },
            "Archived".into(),
            Some(ConversationIntrinsicMutation::Archive(mutation)),
            &Rc::new(RefCell::new(VecDeque::new())),
            &mut |_| {},
        );
        sender.send(result).unwrap();
    });
    assert!(matches!(
        done.recv_timeout(std::time::Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    runtime
        .stop_intrinsic_invocation_as(
            Some(archive.conversation_ref()),
            archive.invocation_ref(),
            &person,
            None,
        )
        .unwrap();
    let fresh = runtime
        .admit_text(Some(archive.conversation_ref()), "@helper fresh work")
        .unwrap();
    drop(held);
    done.recv_timeout(std::time::Duration::from_secs(2)).unwrap().unwrap();
    waiting.join().unwrap();
    assert_eq!(runtime.turn(&fresh.record).unwrap().invocation.unwrap().state,
        ActionInvocationState::Admitted);
    assert!(!runtime.conversation(archive.conversation_ref()).unwrap().unwrap().archived);
}

#[test]
fn archive_reservation_orders_a_delayed_host_before_scheduled_restore() {
    let (_temporary, runtime) = participation_runtime();
    let work = claimed_participation_parent(&runtime);
    let conversation = work.conversation_ref.clone().unwrap();
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let first = runtime.reserve_conversation_archive(&conversation).unwrap();
    let restore = runtime
        .admit_action(
            Some(&conversation),
            "@cwr--restore--conversation",
            json!({}),
        )
        .unwrap();
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(task.record().invocation_ref, restore.invocation_ref().unwrap());
    let (sender, done) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        sender.send(task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})).unwrap();
    });
    assert!(matches!(done.recv_timeout(Duration::from_millis(50)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)));
    runtime.set_conversation_archived_reserved_as(Some(&conversation), &person, true, first).unwrap();
    done.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    worker.join().unwrap();
    assert!(!runtime.conversation(&conversation).unwrap().unwrap().archived);
}

#[test]
fn archive_preserves_preexisting_independent_stop_and_discovery_controls() {
    let (_temporary, runtime) = participation_runtime();
    let work = claimed_participation_parent(&runtime);
    let conversation = work.conversation_ref.as_deref().unwrap();
    let stop = runtime
        .admit_action(
            Some(conversation),
            "@cwr--stop--invocation",
            json!({"invocation_ref": work.invocation_ref}),
        )
        .unwrap();
    let discovery = runtime
        .admit_action(Some(conversation), "@cwr--list--coworkers", json!({}))
        .unwrap();
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let (_, mutation) = runtime.prepare_conversation_archive_as(Some(conversation), &person, true).unwrap();
    let fence = runtime.prepare_archive_stops(&mutation, None).unwrap();
    for control in [&stop, &discovery] {
        assert!(require_no_stop_fence(&runtime.lock().unwrap(), control.invocation_ref().unwrap()).is_ok());
    }
    runtime.lock().unwrap().admission.set_conversation_archived(conversation, &person, true).unwrap();
    drop(fence);
    for control in [&stop, &discovery] {
        assert_eq!(runtime.turn(&control.record).unwrap().invocation.unwrap().state, ActionInvocationState::Admitted);
        runtime.execute_admission(control, |_| {}).unwrap();
        let result = runtime.turn(&control.record).unwrap().invocation.unwrap();
        assert!(result.state.is_terminal());
        assert_eq!(result.attempt_count, 1);
    }
}

#[test]
fn archive_reservation_orders_a_delayed_scheduled_archive_before_host_restore() {
    let (_temporary, runtime) = participation_runtime();
    let archive = runtime.admit_action(None, "@cwr--archive--conversation", json!({})).unwrap();
    let conversation = archive.conversation_ref().to_owned();
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let host_runtime = runtime.clone();
    let host_conversation = conversation.clone();
    let (sender, done) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        sender
            .send(host_runtime.set_conversation_archived_as(
                Some(&host_conversation),
                &person,
                false,
            ))
            .unwrap();
    });
    assert!(matches!(done.recv_timeout(Duration::from_millis(50)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)));
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {}).unwrap();
    // Completion releases order even when the host retains the task object.
    done.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    worker.join().unwrap();
    assert!(!runtime.conversation(&conversation).unwrap().unwrap().archived);
}

#[test]
fn canceling_an_admitted_archive_releases_its_order_without_scheduling_it() {
    let (_temporary, runtime) = participation_runtime();
    let archive = runtime.admit_action(None, "@cwr--archive--conversation", json!({})).unwrap();
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let restore_runtime = runtime.clone();
    let conversation = archive.conversation_ref().to_owned();
    let (sender, done) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        sender.send(restore_runtime.set_conversation_archived_as(Some(&conversation), &person, false)).unwrap();
    });
    assert!(matches!(done.recv_timeout(Duration::from_millis(50)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)));
    runtime.stop_invocation(archive.invocation_ref().unwrap(), None).unwrap();
    done.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    worker.join().unwrap();
    assert!(!runtime.conversation(archive.conversation_ref()).unwrap().unwrap().archived);
}

#[test]
fn canceling_an_archive_waiting_for_its_order_does_not_wait_for_the_prior_control() {
    let (_temporary, runtime) = participation_runtime();
    let work = claimed_participation_parent(&runtime);
    let conversation = work.conversation_ref.as_deref().unwrap();
    let prior = runtime.reserve_conversation_archive(conversation).unwrap();
    let archive = runtime.admit_action(Some(conversation), "@cwr--archive--conversation", json!({})).unwrap();
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    let (sender, done) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        sender.send(task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})).unwrap();
    });
    assert!(matches!(done.recv_timeout(Duration::from_millis(50)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)));
    runtime.stop_invocation(archive.invocation_ref().unwrap(), None).unwrap();
    done.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    worker.join().unwrap();
    assert_eq!(runtime.lock().unwrap().admission.invocation_record(&work.invocation_ref).unwrap().state, ActionInvocationState::Running);
    drop(prior);
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    runtime.set_conversation_archived_as(Some(conversation), &person, false).unwrap();
}
