#[test]
fn nested_archive_completes_its_action_and_stops_parent_and_delegated_work() {
    for own_conversation in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("archive.sqlite");
        let mut store = open(&path);
        let ConversationPreparationOutcome::New(mut root) = prepared_root() else {
            unreachable!()
        };
        root.conversation.owner_context_ref = Some("pack-context".into());
        root.execution_conversation
            .as_mut()
            .unwrap()
            .owner_context_ref = Some("pack-context".into());
        store
            .commit(ConversationPreparationOutcome::New(root))
            .unwrap();
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
        let worker = "cwp://com.example.fixture/actors/worker";
        let archiver = "cwp://com.example.runtime/invocations/archive";
        let delegated = "cwp://com.example.runtime/invocations/delegated";
        for reference in [archiver, delegated] {
            let call = store
                .append_action_call(
                    CONVERSATION_REF,
                    worker,
                    "cwp://com.example.fixture/actors/child",
                    "@child--draft--brief",
                    json!({"topic":"launch"}),
                    "Call child".into(),
                )
                .unwrap();
            let request = child_request(reference, reference);
            let mut adapter = child_adapter_invocation(reference);
            if own_conversation {
                adapter.execution_scope = crate::action_contract::ExecutionScope::OwnConversation;
            }
            store
                .start_child_invocation(request, &call.message_ref, adapter)
                .unwrap();
            let execution = store.execution_conversation_ref(reference).unwrap();
            assert_eq!(
                store
                    .conversation(execution)
                    .unwrap()
                    .owner_context_ref
                    .as_deref(),
                Some("pack-context")
            );
            store.claim_invocation(reference, 21).unwrap();
        }
        let archive = store
            .prepare_conversation_archive(CONVERSATION_REF, worker, true)
            .unwrap();
        store
            .publish_child_action_terminal_with_archive(
                archiver,
                InvocationTerminal {
                    status: ActionResponseStatus::COMPLETED,
                    summary: "Archived".into(),
                    result: Some(json!({"archived":true})),
                    error: None,
                },
                "Archived".into(),
                22,
                archive,
            )
            .unwrap();
        assert_eq!(
            store.invocation_record(archiver).unwrap().state,
            ActionInvocationState::Completed
        );
        for reference in [ROOT_INVOCATION_REF, delegated] {
            assert_eq!(
                store.invocation_record(reference).unwrap().state,
                ActionInvocationState::Stopping
            );
            assert!(store
                .invocation_record(reference)
                .unwrap()
                .terminal_response()
                .is_some());
        }
        assert!(store.conversation(CONVERSATION_REF).unwrap().archived);
        assert!(store.runnable_invocation_refs().is_empty());
        let before = store.conversation(CONVERSATION_REF).unwrap().messages.len();
        assert!(matches!(
            store
                .publish_child_action_terminal(
                    delegated,
                    InvocationTerminal {
                        status: ActionResponseStatus::COMPLETED,
                        summary: "Late completion".into(),
                        result: Some(json!({"late":true})),
                        error: None,
                    },
                    "Late completion".into(),
                    23
                )
                .unwrap(),
            PublishInvocationTerminalOutcome::AlreadyTerminal { .. }
        ));
        assert_eq!(
            store.conversation(CONVERSATION_REF).unwrap().messages.len(),
            before
        );
        drop(store);
        let mut store = open(&path);
        store
            .set_conversation_archived(CONVERSATION_REF, worker, false)
            .unwrap();
        assert!(store.recover_interrupted_invocations().unwrap().is_empty());
        assert_eq!(
            store.invocation_record(archiver).unwrap().state,
            ActionInvocationState::Completed
        );
        assert_eq!(
            store.invocation_record(delegated).unwrap().state,
            ActionInvocationState::Stopping
        );
    }
}
