#[test]
fn archive_recovery_preserves_delegated_before_root_control_order() {
    for (delegated, root, expected_archived) in [
        ("archive--conversation", "restore--conversation", false),
        ("restore--conversation", "archive--conversation", true),
    ] {
        let (temporary, runtime) = participation_runtime();
        let parent = claimed_participation_parent(&runtime);
        let conversation = parent.conversation_ref.clone().unwrap();
        let target = {
            let state = runtime.lock().unwrap();
            exact_action_target(
                &state,
                state
                    .graph
                    .routes
                    .iter()
                    .find(|route| {
                        route.actor_ref == INTRINSIC_CWR_ACTOR_REF && route.action_name == delegated
                    })
                    .unwrap(),
            )
        };
        let call = AdapterCall {
            invocation_ref: parent.invocation_ref.clone(),
            call_id: "delegated-control".into(),
            operation: INVOKE_ACTION_HOST_OPERATION.into(),
            input: json!({"target":target,"input":{}}),
        };
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        runtime.handle_start_action(&call, &parent, &queue).unwrap();
        let child_ref = queue.borrow_mut().pop_front().unwrap();
        let later = runtime
            .admit_action(Some(&conversation), &format!("@cwr--{root}"), json!({}))
            .unwrap();
        let later_ref = later.invocation_ref().unwrap().to_owned();
        let child_sequence = runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(&child_ref)
            .unwrap()
            .admission_sequence;
        let root_sequence = runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(&later_ref)
            .unwrap()
            .admission_sequence;
        assert!(child_sequence < root_sequence);
        let runtime_root = runtime.lock().unwrap().admission.runtime_root_ref().to_owned();
        drop(runtime);
        // Equal timestamps and UUID sort order cannot replace the retained sequence.
        cwpdb_store::test_support::invocation_order::tie_admission_timestamps(
            &temporary.path().join("runtime.sqlite"), &runtime_root, [&child_ref, &later_ref],
        ).unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            fixture_workspace(),
            temporary.path().join("runtime.sqlite"),
            fixture_host_identity(),
        )
        .unwrap();
        let mut first = runtime.try_schedule_execution().unwrap().unwrap();
        assert_eq!(first.record().invocation_ref, child_ref);
        first
            .execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
            .unwrap();
        drop(first);
        let mut second = runtime.try_schedule_execution().unwrap().unwrap();
        assert_eq!(second.record().invocation_ref, later_ref);
        second
            .execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
            .unwrap();
        drop(second);
        assert_eq!(
            runtime
                .conversation(&conversation)
                .unwrap()
                .unwrap()
                .archived,
            expected_archived
        );
        let third = runtime
            .admit_action(
                Some(&conversation),
                "@cwr--restore--conversation",
                json!({}),
            )
            .unwrap();
        assert!(
            runtime
                .lock()
                .unwrap()
                .admission
                .invocation_record(third.invocation_ref().unwrap())
                .unwrap()
                .admission_sequence
                > root_sequence
        );
    }
}
