use crate::v2026_09::host::{
    affected_turn_status_conversations, measure_turn_status_projection,
    project_conversation_turn_statuses_for,
};

#[test]
fn selective_turn_statuses_match_concurrent_roots_without_unrelated_history() {
    use crate::adapter::{ExecutionActivity, ExecutionPhase};
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), temporary.path().join("state.sqlite"), fixture_host_identity(),
    ).unwrap();
    let first = runtime.admit_text(None, "@helper first").unwrap();
    let second = runtime.admit_text(Some(first.conversation_ref()), "@cwr second").unwrap();
    let unrelated = runtime.admit_text(None, "@helper unrelated").unwrap();
    {
        let mut state = runtime.lock().unwrap();
        for (admission, phase) in [
            (&first, ExecutionPhase::Thinking),
            (&second, ExecutionPhase::CompactingContext),
            (&unrelated, ExecutionPhase::RunningCommand),
        ] {
            let reference = admission.invocation_ref().unwrap();
            state.admission.claim_invocation(reference, 10).unwrap();
            state.admission.append_execution_activity(reference,
                ConversationActivityEvent::ExecutionActivity {
                    activity: ExecutionActivity::started(phase),
                },
            ).unwrap();
        }
    }
    let invocations = BTreeSet::from([first.invocation_ref().unwrap().to_owned()]);
    let (full, full_work) = measure_turn_status_projection(|| runtime.conversation_turn_statuses().unwrap());
    let (selected, selected_work) = measure_turn_status_projection(|| runtime.conversation_host()
        .turn_statuses_affected_by(&invocations, &BTreeSet::new()).unwrap());
    assert_eq!(selected, BTreeMap::from([(first.conversation_ref().into(), full[first.conversation_ref()].clone())]));
    assert_eq!(selected[first.conversation_ref()].len(), 2);
    assert_eq!(full_work.len(), 2);
    assert_eq!(selected_work.len(), 1);
    assert_eq!(selected_work[0].0, first.conversation_ref());
    let previous_unrelated_work = full_work.iter().find(|(reference, _)| reference == unrelated.conversation_ref()).unwrap().1;
    {
        let mut state = runtime.lock().unwrap();
        for index in 0..32 {
            state.admission.record_invocation_progress_with_activity(
                unrelated.invocation_ref().unwrap(), 102, format!("Unrelated retained activity {index}"),
            ).unwrap();
        }
    }
    let (_, grown_full_work) = measure_turn_status_projection(|| runtime.conversation_turn_statuses().unwrap());
    let (after, after_work) = measure_turn_status_projection(|| runtime.conversation_host()
        .turn_statuses_affected_by(&invocations, &BTreeSet::new()).unwrap());
    assert_eq!(after, selected);
    assert_eq!(after_work, selected_work);
    let grown_unrelated_work = grown_full_work.iter().find(|(reference, _)| reference == unrelated.conversation_ref()).unwrap().1;
    assert!(grown_unrelated_work > previous_unrelated_work);
    eprintln!("selective turn status histories: full={full_work:?}, grown_full={grown_full_work:?}, selected={selected_work:?}, grown_selected={after_work:?}");
}

#[test]
fn selective_turn_statuses_preserve_synchronous_child_and_terminal_semantics() {
    use crate::adapter::{ExecutionActivity, ExecutionPhase};
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), temporary.path().join("state.sqlite"), fixture_host_identity(),
    ).unwrap();
    let admission = runtime.admit_text(None, "@helper coordinate").unwrap();
    let parent = admission.invocation_ref().unwrap().to_owned();
    let caller = {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claim) = state.admission.claim_invocation(&parent, 10).unwrap() else {
            panic!("claim parent")
        };
        claim.claim.invocation
    };
    let call = AdapterCall {
        invocation_ref: parent.clone(), call_id: "scoped-sync".into(),
        operation: INVOKE_ACTION_HOST_OPERATION.into(), input: json!({}),
    };
    let AdapterHostCallOutcome::Returned(result) = runtime.start_action(
        &call, &caller, &Rc::new(RefCell::new(VecDeque::new())),
        StartActionInput { activation: "@cwr--list--actions".into(), input: json!({}) }, None,
    ).unwrap();
    let child: StartActionResult = serde_json::from_value(result).unwrap();
    {
        let mut state = runtime.lock().unwrap();
        state.admission.claim_invocation(&child.invocation_ref, 20).unwrap();
        state.admission.append_execution_activity(&child.invocation_ref,
            ConversationActivityEvent::ExecutionActivity {
                activity: ExecutionActivity::started(ExecutionPhase::RestoringContext),
            },
        ).unwrap();
    }
    let full = runtime.conversation_turn_statuses().unwrap();
    let changed = BTreeSet::from([child.invocation_ref.clone()]);
    let selected = runtime.conversation_host().turn_statuses_affected_by(&changed, &BTreeSet::new()).unwrap();
    assert_eq!(selected[admission.conversation_ref()], full[admission.conversation_ref()]);
    assert_eq!(selected[admission.conversation_ref()][0].workers.len(), 1);
    assert_eq!(selected[admission.conversation_ref()][0].workers[0].invocation_ref, child.invocation_ref);
    assert_eq!(selected[admission.conversation_ref()][0].workers[0].phase, ExecutionPhase::RestoringContext);
    runtime.stop_invocation(&parent, Some("Stop scoped work".into())).unwrap();
    let (settled, projected) = measure_turn_status_projection(|| runtime.conversation_host()
        .turn_statuses_affected_by(&changed, &BTreeSet::new()).unwrap());
    assert!(settled.contains_key(admission.conversation_ref()));
    assert!(settled.values().all(Vec::is_empty));
    assert!(projected.is_empty());
    assert!(runtime.conversation_turn_statuses().unwrap().is_empty());
}

#[test]
fn selective_turn_status_owners_follow_private_and_forwarded_progress_ancestry() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path_and_options(
        fixture_workspace(), temporary.path().join("state.sqlite"), fixture_host_identity(),
        LocalRuntimeOptions {
            conversation_timeline_rollout: ConversationTimelineRollout::Disabled,
            ..Default::default()
        },
    ).unwrap();
    let root = runtime.admit_text(None, "@helper root").unwrap();
    let other = runtime.admit_text(None, "@helper unrelated").unwrap();
    let parent = root.invocation_ref().unwrap().to_owned();
    let (child, parent_execution, child_execution) = {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claim) = state.admission.claim_invocation(&parent, 10).unwrap() else {
            panic!("claim root")
        };
        let parent_execution = state.admission.execution_conversation_ref(&parent).unwrap().to_owned();
        let record = claim.claim.invocation;
        let child = new_invocation_ref(state.admission.runtime_root_ref());
        let call = state.admission.append_action_call(
            &parent_execution, &record.target_actor_ref, &record.target_actor_ref,
            "@helper--respond_to--message", json!({"source_message_ref":"nested"}), "nested progress".into(),
        ).unwrap();
        let mut adapter = claim.adapter_invocation.clone();
        adapter.invocation_ref = child.clone();
        adapter.conversation_continuity = false;
        adapter.thread = json!({});
        state.admission.start_child_invocation(
            StartActionInvocation {
                invocation_ref: child.clone(),
                origin: InvocationOrigin::HostCall { caller_invocation_ref: parent.clone(), call_id: "scoped-nested".into() },
                requester_actor_ref: record.target_actor_ref.clone(), target_actor_ref: record.target_actor_ref,
                conversation_ref: Some(parent_execution.clone()), actor_integration_ref: record.actor_integration_ref,
                action_name: adapter.action_name.clone(), input: adapter.input.clone(),
                parent_invocation_ref: Some(parent), admitted_at_unix_ms: 20,
            }, &call.message_ref, adapter,
        ).unwrap();
        state.admission.claim_invocation(&child, 21).unwrap();
        state.admission.record_invocation_progress_with_publication(&child, 102, "Nested public progress", true).unwrap();
        let execution = state.admission.execution_conversation_ref(&child).unwrap().to_owned();
        (child, parent_execution, execution)
    };
    let full = runtime.conversation_turn_statuses().unwrap();
    assert!(full.contains_key(other.conversation_ref()));
    for (invocations, conversations) in [
        (BTreeSet::from([child.clone()]), BTreeSet::new()),
        (BTreeSet::new(), BTreeSet::from([parent_execution.clone()])),
        (BTreeSet::new(), BTreeSet::from([child_execution])),
        (BTreeSet::from([child]), BTreeSet::from([root.conversation_ref().to_owned()])),
    ] {
        let state = runtime.lock().unwrap();
        let selected = affected_turn_status_conversations(&state.admission, &invocations, &conversations);
        assert!(selected.contains(root.conversation_ref()));
        assert!(!selected.contains(other.conversation_ref()));
        let (statuses, projected) = measure_turn_status_projection(|| project_conversation_turn_statuses_for(&state.admission, &selected));
        for reference in selected {
            assert_eq!(statuses[&reference], full.get(&reference).cloned().unwrap_or_default());
        }
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].0, root.conversation_ref());
        let public = state.admission.conversation(root.conversation_ref()).unwrap();
        assert!(public.messages.iter().any(|message| message.text == "Nested public progress"));
    }
}

#[test]
fn selective_turn_statuses_keep_completed_conversation_history_cold_after_reopen() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state.sqlite");
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), &path, fixture_host_identity(),
    ).unwrap();
    let completed = runtime.admit_text(None, "@helper completed").unwrap();
    runtime.execute_admission(&completed, |_| {}).unwrap();
    let active = runtime.admit_text(None, "@helper remains active").unwrap();
    let execution = runtime.lock().unwrap().admission
        .execution_conversation_ref(completed.invocation_ref().unwrap()).unwrap().to_owned();
    drop(runtime);
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), &path, fixture_host_identity(),
    ).unwrap();
    assert!(runtime.lock().unwrap().admission
        .hydrated_execution_conversation(completed.invocation_ref().unwrap()).is_none());
    for (invocations, conversations) in [
        (BTreeSet::from([completed.invocation_ref().unwrap().to_owned()]), BTreeSet::new()),
        (BTreeSet::new(), BTreeSet::from([completed.conversation_ref().to_owned()])),
        (BTreeSet::new(), BTreeSet::from([execution])),
    ] {
        let (selected, projected) = measure_turn_status_projection(|| runtime.conversation_host()
            .turn_statuses_affected_by(&invocations, &conversations).unwrap());
        assert!(selected.contains_key(completed.conversation_ref()));
        assert!(selected.values().all(Vec::is_empty));
        assert!(!selected.contains_key(active.conversation_ref()));
        assert!(projected.is_empty());
        let (publication, projected) = measure_turn_status_projection(|| runtime.conversation_host()
            .turn_status_publication_for_execution(&invocations, &conversations, &BTreeMap::new()).unwrap());
        assert!(!publication.replace_all);
        assert_eq!(publication.statuses, selected);
        assert!(projected.is_empty());
    }
    let (full, projected) = measure_turn_status_projection(|| runtime.conversation_turn_statuses().unwrap());
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].0, active.conversation_ref());
    assert!(full.contains_key(active.conversation_ref()));
    assert!(!full.contains_key(completed.conversation_ref()));
    assert!(runtime.lock().unwrap().admission
        .hydrated_execution_conversation(completed.invocation_ref().unwrap()).is_none());
}
