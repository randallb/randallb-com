#[test]
fn archive_stops_active_and_queued_work_without_resuming_on_restore_or_restart() {
    let (temporary, runtime) = participation_runtime();
    let active = claimed_participation_parent(&runtime);
    let conversation = active.conversation_ref.as_deref().unwrap();
    let queued = runtime
        .admit_text(Some(conversation), "@helper queued")
        .unwrap();
    let unrelated = runtime.admit_text(None, "@helper unrelated").unwrap();
    let control = ProcessAdapterStopControl::new();
    let unrelated_control = ProcessAdapterStopControl::new();
    {
        let mut state = runtime.lock().unwrap();
        state
            .active
            .insert(active.invocation_ref.clone(), control.clone());
        state.active.insert(
            unrelated.invocation_ref().unwrap().to_owned(),
            unrelated_control.clone(),
        );
    }
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let before = runtime.conversation(conversation).unwrap().unwrap();
    assert!(runtime
        .set_conversation_archived_as(Some(conversation), "cwp://example.test/stranger", true)
        .is_err());
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(&active.invocation_ref)
            .unwrap()
            .state,
        ActionInvocationState::Running
    );
    let (_, stopped) = runtime
        .set_conversation_archived_with_stops_as(Some(conversation), &person, true)
        .unwrap();
    assert!(control.is_requested());
    assert!(!unrelated_control.is_requested());
    assert!(stopped.contains(&active.invocation_ref));
    assert!(stopped.contains(&queued.invocation_ref().unwrap().to_owned()));
    let after = runtime.conversation(conversation).unwrap().unwrap();
    assert!(after.archived);
    assert_eq!(
        &after.messages[..before.messages.len()],
        before.messages.as_slice()
    );
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(unrelated.invocation_ref().unwrap())
            .unwrap()
            .state,
        ActionInvocationState::Admitted
    );
    runtime
        .set_conversation_archived_as(Some(conversation), &person, false)
        .unwrap();
    assert!(runtime
        .lock()
        .unwrap()
        .admission
        .claim_invocation(&active.invocation_ref, current_unix_ms())
        .is_ok_and(|r| matches!(r, ConversationInvocationClaimOutcome::NotRunnable { .. })));
    let conversation = conversation.to_owned();
    let active_ref = active.invocation_ref.clone();
    let queued_ref = queued.invocation_ref().unwrap().to_owned();
    drop(runtime);
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    for reference in [&active_ref, &queued_ref] {
        assert_eq!(
            runtime
                .lock()
                .unwrap()
                .admission
                .invocation_record(reference)
                .unwrap()
                .state,
            if reference == &active_ref {
                ActionInvocationState::Stopping
            } else {
                ActionInvocationState::Stopped
            }
        );
    }
    assert!(
        !runtime
            .conversation(&conversation)
            .unwrap()
            .unwrap()
            .archived
    );
    let fresh = runtime
        .admit_text(Some(&conversation), "@helper fresh request")
        .unwrap();
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(fresh.invocation_ref().unwrap())
            .unwrap()
            .state,
        ActionInvocationState::Admitted
    );
}

#[test]
fn archive_fence_holds_new_requests_and_blocks_delegation_until_commit() {
    let (_temporary, runtime) = participation_runtime();
    let active = claimed_participation_parent(&runtime);
    let conversation = active.conversation_ref.as_deref().unwrap();
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let (_, mutation) = runtime
        .prepare_conversation_archive_as(Some(conversation), &person, true)
        .unwrap();
    let fence = runtime.prepare_archive_stops(&mutation, None).unwrap();
    let late = runtime
        .admit_text(Some(conversation), "@helper arrived during archive")
        .unwrap();
    {
        let state = runtime.lock().unwrap();
        assert!(require_no_stop_fence(&state, &active.invocation_ref).is_err());
        assert!(require_no_stop_fence(&state, late.invocation_ref().unwrap()).is_err());
    }
    assert!(runtime.try_schedule_execution().unwrap().is_none());
    let before_stops = stopped_archive_snapshot(&runtime.lock().unwrap());
    runtime
        .lock()
        .unwrap()
        .admission
        .set_conversation_archived(conversation, &person, true)
        .unwrap();
    let stopped = new_archive_stops(&runtime.lock().unwrap(), &before_stops);
    runtime.signal_archive_stops(&stopped).unwrap();
    assert!(stopped.contains(&late.invocation_ref().unwrap().to_owned()));
    drop(fence);
    assert!(runtime.lock().unwrap().archiving_conversations.is_empty());
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(late.invocation_ref().unwrap())
            .unwrap()
            .state,
        ActionInvocationState::Stopped
    );
}

fn archive_test_child(
    runtime: &LocalRuntime,
    parent: &ActionInvocationRecord,
    call_id: &str,
) -> String {
    let mut state = runtime.lock().unwrap();
    let conversation = state
        .admission
        .execution_conversation(&parent.invocation_ref)
        .unwrap()
        .conversation_ref
        .clone();
    let reference = new_invocation_ref(state.admission.runtime_root_ref());
    let message = state
        .admission
        .append_host_action_call(ConversationHostActionCall {
            conversation_ref: &conversation,
            caller_invocation_ref: &parent.invocation_ref,
            host_call_id: call_id,
            nested_invocation_ref: &reference,
            caller_actor_ref: &parent.target_actor_ref,
            target_actor_ref: &parent.target_actor_ref,
            activation: "@helper--draft--brief",
            input: json!({}),
            text: "Delegate work".into(),
        })
        .unwrap();
    let mut adapter = AdapterInvocation::new(
        &reference,
        "draft--brief",
        json!({}),
        json!({"type":"object"}),
    );
    adapter.execution_scope = crate::action_contract::ExecutionScope::OwnConversation;
    adapter.response_mode = crate::action_contract::ResponseMode::Async;
    state
        .admission
        .start_child_invocation(
            StartActionInvocation {
                invocation_ref: reference.clone(),
                origin: InvocationOrigin::HostCall {
                    caller_invocation_ref: parent.invocation_ref.clone(),
                    call_id: call_id.into(),
                },
                requester_actor_ref: parent.target_actor_ref.clone(),
                target_actor_ref: parent.target_actor_ref.clone(),
                conversation_ref: Some(conversation),
                actor_integration_ref: parent.actor_integration_ref.clone(),
                action_name: "draft--brief".into(),
                input: json!({}),
                parent_invocation_ref: Some(parent.invocation_ref.clone()),
                admitted_at_unix_ms: current_unix_ms(),
            },
            &message.message_ref,
            adapter,
        )
        .unwrap();
    reference
}

#[test]
fn archive_traverses_completed_ancestors_to_stop_running_and_queued_grandchildren() {
    let (_temporary, runtime) = participation_runtime();
    let root = claimed_participation_parent(&runtime);
    let child_ref = archive_test_child(&runtime, &root, "child");
    let child = {
        let mut state = runtime.lock().unwrap();
        match state
            .admission
            .claim_invocation(&child_ref, current_unix_ms())
            .unwrap()
        {
            ConversationInvocationClaimOutcome::Claimed(claim) => claim.claim.invocation,
            _ => panic!("child not runnable"),
        }
    };
    let active = archive_test_child(&runtime, &child, "active-grandchild");
    let queued = archive_test_child(&runtime, &child, "queued-grandchild");
    let control = ProcessAdapterStopControl::new();
    let before = {
        let mut state = runtime.lock().unwrap();
        state
            .admission
            .claim_invocation(&active, current_unix_ms())
            .unwrap();
        state.active.insert(active.clone(), control.clone());
        state
            .admission
            .publish_child_action_terminal(
                &child_ref,
                InvocationTerminal {
                    status: ActionResponseStatus::COMPLETED,
                    summary: "Delegated".into(),
                    result: Some(json!({})),
                    error: None,
                },
                "Delegated".into(),
                current_unix_ms(),
            )
            .unwrap();
        state
            .admission
            .publish_root_message_completed(
                &root.invocation_ref,
                ActionResponseStatus::COMPLETED,
                json!({"text":"Done"}),
                "Done".into(),
                current_unix_ms(),
            )
            .unwrap();
        assert_ne!(
            state
                .admission
                .invocation_record(&active)
                .unwrap()
                .conversation_ref,
            root.conversation_ref
        );
        [
            state
                .admission
                .invocation_record(&root.invocation_ref)
                .unwrap()
                .clone(),
            state
                .admission
                .invocation_record(&child_ref)
                .unwrap()
                .clone(),
        ]
    };
    let conversation = root.conversation_ref.as_deref().unwrap();
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let (_, mutation) = runtime
        .prepare_conversation_archive_as(Some(conversation), &person, true)
        .unwrap();
    let fence = runtime.prepare_archive_stops(&mutation, None).unwrap();
    {
        let state = runtime.lock().unwrap();
        assert!(require_no_stop_fence(&state, &active).is_err());
        assert!(require_no_stop_fence(&state, &queued).is_err());
    }
    let before_stops = stopped_archive_snapshot(&runtime.lock().unwrap());
    runtime
        .lock()
        .unwrap()
        .admission
        .set_conversation_archived(conversation, &person, true)
        .unwrap();
    let stopped = new_archive_stops(&runtime.lock().unwrap(), &before_stops);
    runtime.signal_archive_stops(&stopped).unwrap();
    drop(fence);
    assert!(control.is_requested());
    for reference in [&active, &queued] {
        assert!(stopped.contains(reference));
        assert_eq!(
            runtime
                .lock()
                .unwrap()
                .admission
                .invocation_record(reference)
                .unwrap()
                .state,
            if reference == &active {
                ActionInvocationState::Stopping
            } else {
                ActionInvocationState::Stopped
            }
        );
    }
    for completed in before {
        assert_eq!(
            runtime
                .lock()
                .unwrap()
                .admission
                .invocation_record(&completed.invocation_ref)
                .unwrap(),
            &completed
        );
    }
}

include!("archive_nested.rs");
