#[test]
fn read_only_open_does_not_recover_an_interrupted_attempt() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    drop(store);

    let reopened = open(&path);
    assert!(
        reopened
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .attempt_active
    );
    drop(reopened);

    let mut recovering = open(&path);
    assert_eq!(
        recovering.recover_interrupted_invocations().unwrap(),
        vec![ROOT_INVOCATION_REF.to_owned()]
    );
    drop(recovering);

    let recovered = open(&path);
    let invocation = recovered.invocation_record(ROOT_INVOCATION_REF).unwrap();
    assert_eq!(invocation.state, ActionInvocationState::Running);
    assert!(!invocation.attempt_active);
    assert_eq!(
        recovered.runnable_invocation_refs(),
        vec![ROOT_INVOCATION_REF.to_owned()]
    );
}
#[test]
fn named_recovery_persists_only_the_selected_interrupted_attempt() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let child_ref = "cwp://com.example.runtime/invocations/live-child";
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let call = store
        .append_action_call(
            CONVERSATION_REF,
            "cwp://com.example.fixture/actors/worker",
            "cwp://com.example.fixture/actors/child",
            "@child--draft--brief",
            json!({"topic": "launch"}),
            "@child--draft--brief".into(),
        )
        .unwrap();
    store
        .start_child_invocation(
            child_request(child_ref, "call-start-live-child"),
            &call.message_ref,
            child_adapter_invocation(child_ref),
        )
        .unwrap();
    store.claim_invocation(child_ref, 12).unwrap();

    assert!(store
        .recover_interrupted_invocation(ROOT_INVOCATION_REF)
        .unwrap());
    assert!(
        !store
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .attempt_active
    );
    assert!(store.invocation_record(child_ref).unwrap().attempt_active);
    drop(store);

    let reopened = open(&path);
    assert!(
        !reopened
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .attempt_active
    );
    assert!(
        reopened
            .invocation_record(child_ref)
            .unwrap()
            .attempt_active
    );
}

#[test]
fn upgrading_suspended_parent_reports_failure_and_preserves_child_result() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("cwpdb.sqlite3");
    let child_ref = "cwp://com.example.runtime/invocations/legacy-child";
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let call = store
        .append_action_call(
            CONVERSATION_REF,
            "cwp://com.example.fixture/actors/worker",
            "cwp://com.example.fixture/actors/child",
            "@child--draft--brief",
            json!({"topic":"launch"}),
            "Draft brief".into(),
        )
        .unwrap();
    store
        .start_child_invocation(
            child_request(child_ref, "legacy-start"),
            &call.message_ref,
            child_adapter_invocation(child_ref),
        )
        .unwrap();
    store.claim_invocation(child_ref, 12).unwrap();
    store
        .publish_child_action_terminal(
            child_ref,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Brief ready".into(),
                result: Some(json!({"artifact_ref":"cwp://com.example.fixture/artifacts/brief"})),
                error: None,
            },
            "Brief ready".into(),
            13,
        )
        .unwrap();
    let child = store.invocation_record(child_ref).unwrap().clone();
    let child_message = store
        .conversation(CONVERSATION_REF)
        .unwrap()
        .messages
        .last()
        .unwrap()
        .clone();
    let mut legacy = serde_json::to_value(&store.state.invocation_ledger).unwrap();
    legacy["invocations"][ROOT_INVOCATION_REF]["attempt_active"] = json!(false);
    legacy["waits"] = json!([{
        "waiter_invocation_ref": ROOT_INVOCATION_REF,
        "awaited_invocation_ref": child_ref,
        "call_id": "legacy-await", "state":"ready",
        "response": child.terminal_response().unwrap(),
    }]);
    let mut staged = store.state.clone();
    staged.invocation_ledger = serde_json::from_value(legacy).unwrap();
    store.replace_state(staged).unwrap();
    drop(store);
    let reopened = open(&path);
    let response = reopened
        .invocation_record(ROOT_INVOCATION_REF)
        .unwrap()
        .terminal_response()
        .unwrap();
    assert_eq!(
        response.error.as_ref().unwrap().code,
        "suspended_execution_unsupported"
    );
    assert_eq!(reopened.invocation_record(child_ref), Some(&child));
    let messages = &reopened.conversation(CONVERSATION_REF).unwrap().messages;
    assert!(messages.contains(&child_message));
    assert!(response
        .summary
        .contains("suspended execution is no longer supported"));
    let messages = messages.clone();
    drop(reopened);
    assert_eq!(
        open(&path).conversation(CONVERSATION_REF).unwrap().messages,
        messages
    );
}

#[test]
fn interrupted_live_host_call_settles_without_restarting_the_parent() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("cwpdb.sqlite3");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
        .append_execution_activity(
            ROOT_INVOCATION_REF,
            ConversationActivityEvent::HostCall {
                call_id: "interrupted-call".into(),
                operation: "cwr.invoke_action".into(),
                input: json!({}),
            },
        )
        .unwrap();
    assert!(store.recover_interrupted_invocations().unwrap().is_empty());
    let parent = store.invocation_record(ROOT_INVOCATION_REF).unwrap();
    assert_eq!(parent.attempt_count, 1);
    assert_eq!(parent.state, ActionInvocationState::Failed);
    assert_eq!(
        parent
            .terminal_response()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .code,
        "execution_interrupted"
    );
    assert!(store.recover_interrupted_invocations().unwrap().is_empty());
}

#[test]
fn recovery_restarts_only_confirmed_pre_live_provider_preparation() {
    for live_started in [None, Some(false), Some(true)] {
        let temporary = tempfile::tempdir().unwrap();
        let mut store = open(&temporary.path().join("cwpdb.sqlite3"));
        store.commit(prepared_root()).unwrap();
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
        store
            .append_execution_activity(
                ROOT_INVOCATION_REF,
                ConversationActivityEvent::ProviderEvent {
                    provider: "cwr".into(),
                    method: "cwr.execution_started".into(),
                    data: json!({}),
                },
            )
            .unwrap();
        if let Some(live_started) = live_started {
            store.append_execution_activity(ROOT_INVOCATION_REF, ConversationActivityEvent::ProviderEvent {
                provider: "codex".into(), method: "cwr.adapter_checkpoint".into(),
                data: json!({"version":1, "invocation_ref":ROOT_INVOCATION_REF, "live_started":live_started}),
            }).unwrap();
        }
        let recovered = store.recover_interrupted_invocations().unwrap();
        if live_started == Some(false) {
            assert_eq!(recovered, [ROOT_INVOCATION_REF]);
            assert!(store
                .invocation_record(ROOT_INVOCATION_REF)
                .unwrap()
                .is_runnable());
        } else {
            assert!(recovered.is_empty());
            assert_eq!(
                store.invocation_record(ROOT_INVOCATION_REF).unwrap().state,
                ActionInvocationState::Failed
            );
        }
    }
}
