#[test]
fn scheduled_work_reserves_capacity_before_allocating_workers() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path_and_options(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
        LocalRuntimeOptions {
            max_concurrent_conversations: NonZeroUsize::MIN,
            ..Default::default()
        },
    )
    .unwrap();
    let first = runtime.admit_text(None, "@helper first").unwrap();
    for _ in 0..40 {
        runtime.admit_text(None, "@helper queued").unwrap();
    }
    let task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(
        task.record().invocation_ref,
        first.invocation_ref().unwrap()
    );
    assert!(runtime.try_schedule_execution().unwrap().is_none());
    assert_eq!(
        runtime
            .turn(&first.record)
            .unwrap()
            .invocation
            .unwrap()
            .attempt_count,
        0
    );

    let control = runtime
        .admit_text(Some(first.conversation_ref()), "@cwr help")
        .unwrap();
    let mut control_task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(
        control_task.record().invocation_ref,
        control.invocation_ref().unwrap()
    );
    assert!(runtime.try_schedule_execution().unwrap().is_none());
    control_task
        .execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
        .unwrap();
    drop(control_task);
    assert!(runtime.try_schedule_execution().unwrap().is_none());

    // A worker that fails to spawn releases its reservation without claiming
    // the invocation, leaving that same request eligible for retry.
    drop(task);
    let mut retry = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(
        retry.record().invocation_ref,
        first.invocation_ref().unwrap()
    );
    runtime.control_drain("begin").unwrap();
    retry
        .execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
        .unwrap();
    drop(retry);
    assert_eq!(
        runtime
            .turn(&first.record)
            .unwrap()
            .invocation
            .unwrap()
            .attempt_count,
        0
    );
    assert!(runtime.try_schedule_execution().unwrap().is_none());
    runtime.control_drain("cancel").unwrap();
    assert!(runtime.try_schedule_execution().unwrap().is_some());
    runtime.shutdown(Duration::from_secs(1)).unwrap();
    assert!(runtime.try_schedule_execution().unwrap().is_none());
}

#[test]
fn queued_work_in_a_busy_conversation_does_not_take_other_slots() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path_and_options(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
        LocalRuntimeOptions {
            max_concurrent_conversations: NonZeroUsize::new(2).unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
    let first = runtime.admit_text(None, "@helper first").unwrap();
    runtime
        .admit_text(Some(first.conversation_ref()), "@helper second")
        .unwrap();
    let other = runtime.admit_text(None, "@helper independent").unwrap();
    let first_task = runtime.try_schedule_execution().unwrap().unwrap();
    let second_task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(
        first_task.record().invocation_ref,
        first.invocation_ref().unwrap()
    );
    assert_eq!(
        second_task.record().invocation_ref,
        other.invocation_ref().unwrap()
    );
    assert!(runtime.try_schedule_execution().unwrap().is_none());
}

#[test]
fn scheduler_finds_queued_admissions_after_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("runtime.sqlite");
    let runtime =
        LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity())
            .unwrap();
    let queued = runtime.admit_text(None, "@helper retained queue").unwrap();
    drop(runtime);
    let reopened =
        LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity())
            .unwrap();
    let task = reopened.try_schedule_execution().unwrap().unwrap();
    assert_eq!(
        task.record().invocation_ref,
        queued.invocation_ref().unwrap()
    );
    assert_eq!(task.record().attempt_count, 0);
}

#[test]
fn completed_admission_replay_does_not_borrow_cold_execution_history() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("runtime.sqlite");
    let runtime =
        LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity())
            .unwrap();
    let admission = runtime.admit_text(None, "@helper completed").unwrap();
    runtime.execute_admission(&admission, |_| {}).unwrap();
    let completed = runtime.turn(&admission.record).unwrap();
    assert!(!runtime
        .lock()
        .unwrap()
        .admission
        .invocation_record(admission.invocation_ref().unwrap())
        .unwrap()
        .is_runnable());
    drop(runtime);
    let reopened =
        LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity())
            .unwrap();
    assert!(reopened
        .lock()
        .unwrap()
        .admission
        .hydrated_execution_conversation(admission.invocation_ref().unwrap())
        .is_none());
    let replayed = reopened.execute_admission(&admission, |_| {}).unwrap();
    assert_eq!(replayed.admission, completed.admission);
}

fn stage_async_own_conversation_child(runtime: &LocalRuntime, source: &LocalAdmission) -> String {
    let parent_ref = source.invocation_ref().unwrap().to_owned();
    let mut state = runtime.lock().unwrap();
    let parent = match state.admission.claim_invocation(&parent_ref, 10).unwrap() {
        ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
        ConversationInvocationClaimOutcome::NotRunnable { .. } => {
            panic!("source invocation was not runnable")
        }
    };
    let child_ref = new_invocation_ref(state.admission.runtime_root_ref());
    let call_message = state
        .admission
        .append_host_action_call(ConversationHostActionCall {
            conversation_ref: source.conversation_ref(),
            caller_invocation_ref: &parent_ref,
            host_call_id: "async-own-child",
            nested_invocation_ref: &child_ref,
            caller_actor_ref: &parent.target_actor_ref,
            target_actor_ref: &parent.target_actor_ref,
            activation: "@helper--long--task",
            input: json!({}),
            text: "@helper--long--task".into(),
        })
        .unwrap();
    let mut invocation = AdapterInvocation::new(
        child_ref.clone(),
        "long--task",
        json!({}),
        json!({"type": "object"}),
    );
    invocation.execution_scope = crate::action_contract::ExecutionScope::OwnConversation;
    invocation.response_mode = crate::action_contract::ResponseMode::Async;
    state
        .admission
        .start_child_invocation(
            StartActionInvocation {
                invocation_ref: child_ref.clone(),
                origin: InvocationOrigin::HostCall {
                    caller_invocation_ref: parent_ref.clone(),
                    call_id: "async-own-child".into(),
                },
                requester_actor_ref: parent.target_actor_ref.clone(),
                target_actor_ref: parent.target_actor_ref.clone(),
                conversation_ref: Some(source.conversation_ref().into()),
                actor_integration_ref: parent.actor_integration_ref.clone(),
                action_name: "long--task".into(),
                input: json!({}),
                parent_invocation_ref: Some(parent_ref),
                admitted_at_unix_ms: 20,
            },
            &call_message.message_ref,
            invocation,
        )
        .unwrap();
    child_ref
}

fn copy_scheduling_fixture(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_scheduling_fixture(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn async_own_conversation_action_does_not_reserve_its_source_conversation() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    copy_scheduling_fixture(fixture_workspace().parent().unwrap(), &workspace);
    let helper = workspace.join("coworkers/helper/coworker.toml");
    fs::write(
        &helper,
        format!(
            "{}\nmessage_mode = \"actions_only\"\n",
            fs::read_to_string(&helper).unwrap()
        ),
    )
    .unwrap();
    let manifest = workspace.join("CWR.toml");
    let host = LocalHostIdentity::new(
        load_default_local_acting_actor_ref(&manifest).unwrap(),
        fixture_host_identity().ingress_integration_ref,
    );
    let runtime = LocalRuntime::open_with_state_path_and_options(
        manifest,
        temporary.path().join("runtime.sqlite"),
        host,
        LocalRuntimeOptions {
            max_concurrent_conversations: NonZeroUsize::new(2).unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
    for binding in &mut runtime.lock().unwrap().graph.actor_integrations {
        binding.adapter_ref = "cwp://example.test/adapters/unavailable".into();
    }
    let source = runtime.admit_text(None, "@helper delegate").unwrap();
    let child_ref = stage_async_own_conversation_child(&runtime, &source);
    let follow_up = runtime
        .admit_text(Some(source.conversation_ref()), "@helper follow up")
        .unwrap();
    runtime
        .admit_text(
            Some(source.conversation_ref()),
            "@helper ordered after follow up",
        )
        .unwrap();

    let first_task = runtime.try_schedule_execution().unwrap().unwrap();
    let second_task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(
        BTreeSet::from([
            first_task.record().invocation_ref.clone(),
            second_task.record().invocation_ref.clone(),
        ]),
        BTreeSet::from([
            child_ref.clone(),
            follow_up.invocation_ref().unwrap().to_owned(),
        ])
    );
    assert!(runtime.try_schedule_execution().unwrap().is_none());

    let (mut child_task, mut follow_up_task) = if first_task.record().invocation_ref == child_ref {
        (first_task, second_task)
    } else {
        (second_task, first_task)
    };
    let (child_started, child_starts) = std::sync::mpsc::channel();
    let (release_child, child_release) = std::sync::mpsc::channel();
    thread::scope(|scope| {
        let blocked_child_ref = child_ref.clone();
        let child_worker = scope.spawn(move || {
            child_task
                .execute_with_model_output(&AdapterModelOutputMailbox::discard(), |event| {
                    if matches!(
                        event,
                        LocalRuntimeEvent::InvocationStarted { ref invocation_ref, .. }
                            if invocation_ref == &blocked_child_ref
                    ) {
                        child_started.send(()).unwrap();
                        child_release.recv_timeout(Duration::from_secs(2)).unwrap();
                    }
                })
                .unwrap();
        });
        child_starts.recv_timeout(Duration::from_secs(2)).unwrap();
        follow_up_task
            .execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
            .unwrap();
        let state = runtime.lock().unwrap();
        assert_eq!(
            state
                .admission
                .invocation_record(follow_up.invocation_ref().unwrap())
                .unwrap()
                .state,
            ActionInvocationState::Completed
        );
        assert_eq!(
            state.admission.invocation_record(&child_ref).unwrap().state,
            ActionInvocationState::Running
        );
        drop(state);
        release_child.send(()).unwrap();
        child_worker.join().unwrap();
    });
}

#[test]
fn async_own_conversation_group_is_rebuilt_from_retained_contract() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("runtime.sqlite");
    let (source_conversation_ref, source_invocation_ref, child_ref) = {
        let runtime = LocalRuntime::open_with_state_path_and_options(
            fixture_workspace(),
            &path,
            fixture_host_identity(),
            LocalRuntimeOptions {
                max_concurrent_conversations: NonZeroUsize::new(2).unwrap(),
                ..Default::default()
            },
        )
        .unwrap();
        let source = runtime.admit_text(None, "@helper delegate").unwrap();
        let child_ref = stage_async_own_conversation_child(&runtime, &source);
        (
            source.conversation_ref().to_owned(),
            source.invocation_ref().unwrap().to_owned(),
            child_ref,
        )
    };

    let reopened = LocalRuntime::open_with_state_path_and_options(
        fixture_workspace(),
        &path,
        fixture_host_identity(),
        LocalRuntimeOptions {
            max_concurrent_conversations: NonZeroUsize::new(2).unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
    reopened
        .admit_text(Some(&source_conversation_ref), "@helper follow up")
        .unwrap();
    let first_task = reopened.try_schedule_execution().unwrap().unwrap();
    let second_task = reopened.try_schedule_execution().unwrap().unwrap();
    assert_eq!(
        BTreeSet::from([
            first_task.record().invocation_ref.clone(),
            second_task.record().invocation_ref.clone(),
        ]),
        BTreeSet::from([child_ref, source_invocation_ref])
    );
}

#[test]
fn new_and_replayed_async_own_children_stay_off_the_callers_queue() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let source = runtime.admit_text(None, "@helper delegate").unwrap();
    let child_ref = stage_async_own_conversation_child(&runtime, &source);
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    let caller = runtime
        .lock()
        .unwrap()
        .admission
        .invocation_record(source.invocation_ref().unwrap())
        .unwrap()
        .clone();
    let call = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "async-own-child".into(),
        operation: START_ACTION_HOST_OPERATION.into(),
        input: json!({"activation":"@helper--long--task","input":{}}),
    };
    let outcome = runtime
        .start_action(
            &call,
            &caller,
            &queue,
            StartActionInput {
                activation: "@helper--long--task".into(),
                input: json!({}),
            },
            None,
        )
        .unwrap();

    assert!(queue.borrow().is_empty());
    assert_eq!(
        serde_json::from_value::<StartActionResult>(match outcome {
            AdapterHostCallOutcome::Returned(value) => value,
        })
        .unwrap()
        .invocation_ref,
        child_ref
    );
}

#[test]
fn nested_sync_work_inherits_ownership_and_async_work_starts_a_new_group() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let source = runtime.admit_text(None, "@helper delegate").unwrap();
    let parent_ref = stage_async_own_conversation_child(&runtime, &source);
    let mut state = runtime.lock().unwrap();
    let parent = match state.admission.claim_invocation(&parent_ref, 30).unwrap() {
        ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
        ConversationInvocationClaimOutcome::NotRunnable { .. } => {
            panic!("independent parent was not runnable")
        }
    };
    let parent_group = execution_group_ref(&state.admission, &parent)
        .unwrap()
        .to_owned();
    let mut child_refs = Vec::new();
    for (call_id, response_mode) in [
        ("sync-child", crate::action_contract::ResponseMode::Sync),
        ("async-child", crate::action_contract::ResponseMode::Async),
    ] {
        let child_ref = new_invocation_ref(state.admission.runtime_root_ref());
        let call_message = state
            .admission
            .append_host_action_call(ConversationHostActionCall {
                conversation_ref: &parent_group,
                caller_invocation_ref: &parent_ref,
                host_call_id: call_id,
                nested_invocation_ref: &child_ref,
                caller_actor_ref: &parent.target_actor_ref,
                target_actor_ref: &parent.target_actor_ref,
                activation: "@helper--nested--task",
                input: json!({}),
                text: "@helper--nested--task".into(),
            })
            .unwrap();
        let mut invocation = AdapterInvocation::new(
            child_ref.clone(),
            "nested--task",
            json!({}),
            json!({"type": "object"}),
        );
        invocation.execution_scope = crate::action_contract::ExecutionScope::OwnConversation;
        invocation.response_mode = response_mode;
        state
            .admission
            .start_child_invocation(
                StartActionInvocation {
                    invocation_ref: child_ref.clone(),
                    origin: InvocationOrigin::HostCall {
                        caller_invocation_ref: parent_ref.clone(),
                        call_id: call_id.into(),
                    },
                    requester_actor_ref: parent.target_actor_ref.clone(),
                    target_actor_ref: parent.target_actor_ref.clone(),
                    conversation_ref: Some(parent_group.clone()),
                    actor_integration_ref: parent.actor_integration_ref.clone(),
                    action_name: "nested--task".into(),
                    input: json!({}),
                    parent_invocation_ref: Some(parent_ref.clone()),
                    admitted_at_unix_ms: 40 + child_refs.len() as u64,
                },
                &call_message.message_ref,
                invocation,
            )
            .unwrap();
        child_refs.push(child_ref);
    }
    {
        let sync_child = state.admission.invocation_record(&child_refs[0]).unwrap();
        let async_child = state.admission.invocation_record(&child_refs[1]).unwrap();
        assert_eq!(
            execution_group_ref(&state.admission, sync_child),
            Some(parent_group.as_str())
        );
        assert_ne!(
            execution_group_ref(&state.admission, async_child),
            Some(parent_group.as_str())
        );
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        LocalRuntime::enqueue_with_current_worker(&state.admission, sync_child, &queue);
        LocalRuntime::enqueue_with_current_worker(&state.admission, async_child, &queue);
        assert_eq!(
            queue.borrow().iter().cloned().collect::<Vec<_>>(),
            vec![child_refs[0].clone()]
        );
    }
    let sync_parent = match state
        .admission
        .claim_invocation(&child_refs[0], 50)
        .unwrap()
    {
        ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
        ConversationInvocationClaimOutcome::NotRunnable { .. } => {
            panic!("synchronous child was not runnable")
        }
    };
    let grandchild_ref = new_invocation_ref(state.admission.runtime_root_ref());
    let call_message = state
        .admission
        .append_host_action_call(ConversationHostActionCall {
            conversation_ref: &parent_group,
            caller_invocation_ref: &child_refs[0],
            host_call_id: "sync-grandchild",
            nested_invocation_ref: &grandchild_ref,
            caller_actor_ref: &sync_parent.target_actor_ref,
            target_actor_ref: &sync_parent.target_actor_ref,
            activation: "@helper--nested--grandchild",
            input: json!({}),
            text: "@helper--nested--grandchild".into(),
        })
        .unwrap();
    let mut grandchild = AdapterInvocation::new(
        grandchild_ref.clone(),
        "nested--grandchild",
        json!({}),
        json!({"type": "object"}),
    );
    grandchild.execution_scope = crate::action_contract::ExecutionScope::OwnConversation;
    grandchild.response_mode = crate::action_contract::ResponseMode::Sync;
    state
        .admission
        .start_child_invocation(
            StartActionInvocation {
                invocation_ref: grandchild_ref.clone(),
                origin: InvocationOrigin::HostCall {
                    caller_invocation_ref: child_refs[0].clone(),
                    call_id: "sync-grandchild".into(),
                },
                requester_actor_ref: sync_parent.target_actor_ref.clone(),
                target_actor_ref: sync_parent.target_actor_ref.clone(),
                conversation_ref: Some(parent_group.clone()),
                actor_integration_ref: sync_parent.actor_integration_ref.clone(),
                action_name: "nested--grandchild".into(),
                input: json!({}),
                parent_invocation_ref: Some(child_refs[0].clone()),
                admitted_at_unix_ms: 50,
            },
            &call_message.message_ref,
            grandchild,
        )
        .unwrap();
    assert_eq!(
        execution_group_ref(
            &state.admission,
            state.admission.invocation_record(&grandchild_ref).unwrap()
        ),
        Some(parent_group.as_str())
    );
    assert!(belongs_to_independently_scheduled_group(
        &state.admission,
        state.admission.invocation_record(&grandchild_ref).unwrap()
    ));
}
