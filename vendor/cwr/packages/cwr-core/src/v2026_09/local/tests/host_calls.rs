#[test]
fn child_invocation_source_is_the_coworker_that_called_the_action() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "@helper coordinate").unwrap();
    let parent_ref = admission.invocation_ref().unwrap().to_owned();
    let caller = {
        let mut state = runtime.lock().unwrap();
        match state.admission.claim_invocation(&parent_ref, 10).unwrap() {
            ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
            ConversationInvocationClaimOutcome::NotRunnable { .. } => {
                panic!("new invocation was not runnable")
            }
        }
    };
    let call = AdapterCall {
        invocation_ref: parent_ref,
        call_id: "list-actions".into(),
        operation: START_ACTION_HOST_OPERATION.into(),
        input: serde_json::to_value(StartActionInput {
            activation: "@cwr--list--actions".into(),
            input: json!({}),
        })
        .unwrap(),
    };
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    let AdapterHostCallOutcome::Returned(result) =
        runtime.handle_start_action(&call, &caller, &queue).unwrap();
    let child: StartActionResult = serde_json::from_value(result).unwrap();
    let mut state = runtime.lock().unwrap();
    let claimed = match state
        .admission
        .claim_invocation(&child.invocation_ref, 20)
        .unwrap()
    {
        ConversationInvocationClaimOutcome::Claimed(claimed) => claimed,
        ConversationInvocationClaimOutcome::NotRunnable { .. } => {
            panic!("child invocation was not runnable")
        }
    };

    assert_eq!(
        claimed.adapter_invocation.source_actor_ref.as_deref(),
        Some(crate::v2026_09::test_fixture::actor_ref("helper"))
    );
    let conversation = state
        .admission
        .conversation(admission.conversation_ref())
        .unwrap();
    let action_call = conversation.messages.last().unwrap();
    assert_eq!(
        action_call.author_actor_ref,
        crate::v2026_09::test_fixture::actor_ref("helper")
    );
}

#[test]
fn stop_host_call_replays_after_publication_without_a_result() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "@helper stop me").unwrap();
    let invocation_ref = admission.invocation_ref().unwrap().to_owned();
    let caller = {
        let mut state = runtime.lock().unwrap();
        match state
            .admission
            .claim_invocation(&invocation_ref, 10)
            .unwrap()
        {
            ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
            ConversationInvocationClaimOutcome::NotRunnable { .. } => {
                panic!("new invocation was not runnable")
            }
        }
    };
    let call = AdapterCall {
        invocation_ref: invocation_ref.clone(),
        call_id: "stop-call".into(),
        operation: STOP_INVOCATION_HOST_OPERATION.into(),
        input: serde_json::to_value(StopInvocationInput {
            invocation_ref: invocation_ref.clone(),
            reason: Some("requested".into()),
        })
        .unwrap(),
    };
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    let first = runtime
        .handle_stop_invocation(&call, &caller, &queue)
        .unwrap();
    let message_count = runtime
        .conversation(admission.conversation_ref())
        .unwrap()
        .unwrap()
        .messages
        .len();

    let replay = runtime
        .handle_stop_invocation(&call, &caller, &queue)
        .unwrap();
    assert_eq!(replay, first);
    assert_eq!(
        runtime
            .conversation(admission.conversation_ref())
            .unwrap()
            .unwrap()
            .messages
            .len(),
        message_count
    );
    let state = runtime.lock().unwrap();
    let host_calls = state
        .admission
        .execution_conversation(&invocation_ref)
        .unwrap()
        .activity
        .iter()
        .filter(|activity| {
            matches!(
                &activity.event,
                ConversationActivityEvent::HostCall { call_id, .. }
                    if call_id == "stop-call"
            )
        })
        .count();
    assert_eq!(host_calls, 1);
}

#[test]
fn host_projection_marks_detached_child_without_a_wait() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let (parent_ref, child_ref) = detached_parent_and_child(&runtime);
    let conversation_ref = runtime
        .conversation_ref_for_invocation(&parent_ref)
        .unwrap()
        .unwrap();

    let projected = runtime
        .host_conversation(&conversation_ref)
        .unwrap()
        .unwrap();
    let relation = projected
        .activity
        .iter()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::WorkUpdate {
                invocation_ref,
                parent_invocation_ref,
                wait,
                ..
            } if invocation_ref == &child_ref => Some((parent_invocation_ref, wait)),
            _ => None,
        })
        .expect("projected detached child relationship");
    assert_eq!(relation.0.as_deref(), Some(parent_ref.as_str()));
    assert_eq!(relation.1, &None);
}

#[test]
fn turn_status_separates_root_control_from_concurrent_workers() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let (parent_ref, child_ref) = detached_parent_and_child(&runtime);
    {
        let mut state = runtime.lock().unwrap();
        match state.admission.claim_invocation(&child_ref, 20).unwrap() {
            ConversationInvocationClaimOutcome::Claimed(_) => {}
            ConversationInvocationClaimOutcome::NotRunnable { .. } => {
                panic!("new child invocation was not runnable")
            }
        }
    }

    let statuses = runtime.conversation_turn_statuses().unwrap();
    let turn = statuses
        .values()
        .flatten()
        .find(|turn| turn.invocation_ref == parent_ref)
        .expect("root turn status");
    assert_eq!(turn.invocation_ref, parent_ref);
    assert_eq!(turn.workers.len(), 2);
    let parent = turn
        .workers
        .iter()
        .find(|worker| worker.invocation_ref == parent_ref)
        .expect("parent worker");
    assert_eq!(parent.started_at_unix_ms, Some(10));
    assert_eq!(parent.phase, ConversationWorkerPhase::Working);
    let child = turn
        .workers
        .iter()
        .find(|worker| worker.invocation_ref == child_ref)
        .expect("child worker");
    assert_eq!(child.started_at_unix_ms, Some(20));
    assert_eq!(child.phase, ConversationWorkerPhase::Working);
}

#[test]
fn active_stop_targets_the_ingress_root_instead_of_waking_its_parent() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "@helper coordinate").unwrap();
    let parent_ref = admission.invocation_ref().unwrap().to_owned();
    let child_ref = {
        let mut state = runtime.lock().unwrap();
        let parent = match state.admission.claim_invocation(&parent_ref, 10).unwrap() {
            ConversationInvocationClaimOutcome::Claimed(claimed) => claimed.claim.invocation,
            ConversationInvocationClaimOutcome::NotRunnable { .. } => {
                panic!("new invocation was not runnable")
            }
        };
        let child_ref = new_invocation_ref(state.admission.runtime_root_ref());
        let call_message = state
            .admission
            .append_host_action_call(ConversationHostActionCall {
                conversation_ref: admission.conversation_ref(),
                caller_invocation_ref: &parent_ref,
                host_call_id: "start-child",
                nested_invocation_ref: &child_ref,
                caller_actor_ref: &parent.target_actor_ref,
                target_actor_ref: &parent.target_actor_ref,
                activation: "@helper--draft--brief",
                input: json!({}),
                text: "@helper--draft--brief".into(),
            })
            .unwrap();
        state
            .admission
            .start_child_invocation(
                StartActionInvocation {
                    invocation_ref: child_ref.clone(),
                    origin: InvocationOrigin::HostCall {
                        caller_invocation_ref: parent_ref.clone(),
                        call_id: "start-child".into(),
                    },
                    requester_actor_ref: parent.target_actor_ref.clone(),
                    target_actor_ref: parent.target_actor_ref.clone(),
                    conversation_ref: Some(admission.conversation_ref().into()),
                    actor_integration_ref: parent.actor_integration_ref.clone(),
                    action_name: "draft--brief".into(),
                    input: json!({}),
                    parent_invocation_ref: Some(parent_ref.clone()),
                    admitted_at_unix_ms: 20,
                },
                &call_message.message_ref,
                AdapterInvocation::new(
                    child_ref.clone(),
                    "draft--brief",
                    json!({}),
                    json!({"type": "object"}),
                ),
            )
            .unwrap();

        child_ref
    };

    let response = runtime
        .stop_active_intrinsic_invocation(&child_ref, Some("interrupted from a local host".into()))
        .unwrap();
    let crate::intrinsic_cwr::IntrinsicCwrResponse::InvocationStopped {
        invocation_ref,
        stopped_invocation_refs,
    } = response
    else {
        panic!("active stop was not accepted")
    };
    assert_eq!(invocation_ref, parent_ref);
    assert!(stopped_invocation_refs.contains(&parent_ref));
    assert!(stopped_invocation_refs.contains(&child_ref));
    let state = runtime.lock().unwrap();
    assert_eq!(
        state
            .admission
            .invocation_record(&parent_ref)
            .unwrap()
            .state,
        ActionInvocationState::Stopping
    );
    assert_eq!(
        state.admission.invocation_record(&child_ref).unwrap().state,
        ActionInvocationState::Stopped
    );
    assert!(!state.pending_invocation_refs.contains(&parent_ref));
}
#[test]
fn lean_bootstrap_size_does_not_follow_retained_history() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let first = runtime.admit_text(None, "@helper begin").unwrap();
    for index in 0..100 {
        runtime
            .admit_text(
                Some(first.conversation_ref()),
                format!("@helper retained message {index}: {}", "x".repeat(256)),
            )
            .unwrap();
    }

    let full = runtime.host_conversations().unwrap();
    let bootstrap = runtime.host_conversation_bootstrap(false).unwrap();
    let full_root = full
        .iter()
        .find(|conversation| conversation.conversation_ref == first.conversation_ref())
        .unwrap();
    let bootstrap_root = bootstrap
        .iter()
        .find(|conversation| conversation.conversation_ref == first.conversation_ref())
        .unwrap();
    let full_bytes = serde_json::to_vec(full_root).unwrap().len();
    let bootstrap_bytes = serde_json::to_vec(bootstrap_root).unwrap().len();

    assert!(
        full_bytes > bootstrap_bytes * 50,
        "{full_bytes} <= {bootstrap_bytes} * 50"
    );
    assert!(bootstrap
        .iter()
        .all(|conversation| conversation.messages.is_empty()));
    assert_eq!(bootstrap_root.conversation_ref, full_root.conversation_ref);
    assert_eq!(
        bootstrap_root.participant_actor_refs,
        full_root.participant_actor_refs
    );
}
#[test]
fn lean_bootstrap_retains_latest_terminal_root_work() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("conversation-admission.json"),
        fixture_host_identity(),
    )
    .unwrap();
    let admission = runtime.admit_text(None, "Initial title").unwrap();
    let completion = runtime
        .admit_action(
            Some(admission.conversation_ref()),
            crate::intrinsic_cwr::RENAME_CONVERSATION_ACTIVATION,
            json!({"title": "Finished title"}),
        )
        .unwrap();
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(
        task.record().invocation_ref,
        completion.invocation_ref().unwrap()
    );
    let running_bootstrap = runtime.host_conversation_bootstrap(false).unwrap();
    let running_conversation = running_bootstrap
        .iter()
        .find(|conversation| conversation.conversation_ref == admission.conversation_ref())
        .unwrap();
    assert!(running_conversation.activity.is_empty());
    task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
        .unwrap();

    let bootstrap = runtime.host_conversation_bootstrap(false).unwrap();
    let conversation = bootstrap
        .iter()
        .find(|conversation| conversation.conversation_ref == admission.conversation_ref())
        .unwrap();
    assert!(conversation.messages.is_empty());
    assert!(conversation.activity.iter().any(|activity| matches!(
        &activity.event,
        ConversationActivityEvent::WorkLifecycle {
            parent_invocation_ref: None,
            state: ActionInvocationState::Completed,
            ..
        }
    )));
    assert!(conversation.activity.iter().all(|activity| matches!(
        &activity.event,
        ConversationActivityEvent::WorkLifecycle {
            parent_invocation_ref: None,
            state: ActionInvocationState::Completed,
            ..
        }
    )));
    drop(task);

    let later = runtime
        .admit_action(
            Some(admission.conversation_ref()),
            crate::intrinsic_cwr::RENAME_CONVERSATION_ACTIVATION,
            json!({"title": "Later title"}),
        )
        .unwrap();
    runtime
        .try_schedule_execution()
        .unwrap()
        .unwrap()
        .execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
        .unwrap();
    let bootstrap = runtime.host_conversation_bootstrap(false).unwrap();
    let conversation = bootstrap
        .iter()
        .find(|conversation| conversation.conversation_ref == admission.conversation_ref())
        .unwrap();
    assert_eq!(conversation.activity.len(), 1);
    let selected = match &conversation.activity[0].event {
        ConversationActivityEvent::WorkLifecycle { invocation_ref, .. } => invocation_ref,
        _ => unreachable!(),
    };
    assert_eq!(selected, later.invocation_ref().unwrap());

    let failed = runtime
        .admit_text(Some(admission.conversation_ref()), "@helper fail this turn")
        .unwrap();
    let failed_ref = failed.invocation_ref().unwrap().to_owned();
    {
        let mut state = runtime.lock().unwrap();
        assert!(matches!(
            state.admission.claim_invocation(&failed_ref, 100).unwrap(),
            ConversationInvocationClaimOutcome::Claimed(_)
        ));
        state
            .admission
            .fail(&failed_ref, "Expected failure".into())
            .unwrap();
    }
    let bootstrap = runtime.host_conversation_bootstrap(false).unwrap();
    let conversation = bootstrap
        .iter()
        .find(|conversation| conversation.conversation_ref == admission.conversation_ref())
        .unwrap();
    assert_eq!(conversation.activity.len(), 1);
    assert!(matches!(&conversation.activity[0].event,
            ConversationActivityEvent::WorkLifecycle {
                invocation_ref, state: ActionInvocationState::Failed, ..
            } if invocation_ref == &failed_ref));
    }
