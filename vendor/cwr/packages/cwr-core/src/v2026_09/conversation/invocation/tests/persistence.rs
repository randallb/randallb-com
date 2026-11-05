fn open(path: &Path) -> ConversationAdmissionStore {
    ConversationAdmissionStore::open(path, RUNTIME_ROOT).expect("open conversation store")
}

fn root_adapter_invocation() -> AdapterInvocation {
    let mut invocation = AdapterInvocation::new(
        ROOT_INVOCATION_REF,
        RESPOND_TO_MESSAGE_ACTION_NAME,
        json!({"source_message_ref": SOURCE_MESSAGE_REF}),
        json!({"type": "object"}),
    );
    invocation.integration_ref = Some("cwp://com.example.fixture/integrations/codex".into());
    invocation
}

#[test]
fn execution_activity_persist_failure_does_not_mutate_live_state() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let before = store
        .execution_conversation(ROOT_INVOCATION_REF)
        .unwrap()
        .activity
        .to_vec();
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, true).unwrap();

    assert!(store
        .append_execution_activity(
            ROOT_INVOCATION_REF,
            ConversationActivityEvent::HostCall {
                call_id: "call-failed-write".into(),
                operation: "cwr.start_action".into(),
                input: json!({}),
            },
        )
        .is_err());

    assert_eq!(
        store
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .activity,
        before
    );
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, false).unwrap();
}

#[test]
fn host_action_call_persist_failure_does_not_mutate_live_state() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let messages_before = store
        .conversation(CONVERSATION_REF)
        .unwrap()
        .messages
        .to_vec();
    let activity_before = store
        .execution_conversation(ROOT_INVOCATION_REF)
        .unwrap()
        .activity
        .to_vec();
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, true).unwrap();

    assert!(store
        .append_host_action_call(ConversationHostActionCall {
            conversation_ref: CONVERSATION_REF,
            caller_invocation_ref: ROOT_INVOCATION_REF,
            host_call_id: "call-failed-write",
            nested_invocation_ref: "cwp://com.example.runtime/invocations/child-failed-write",
            caller_actor_ref: "cwp://com.example.fixture/actors/worker",
            target_actor_ref: "cwp://com.example.fixture/actors/child",
            activation: "@child--draft--brief",
            input: json!({"topic": "launch"}),
            text: "@child--draft--brief".into(),
        })
        .is_err());

    assert_eq!(
        store.conversation(CONVERSATION_REF).unwrap().messages,
        messages_before
    );
    assert_eq!(
        store
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .activity,
        activity_before
    );
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, false).unwrap();
}

#[test]
fn lifecycle_start_write_failure_exposes_no_call_receipt_message_or_child() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let child_ref = "cwp://com.example.runtime/invocations/child-atomic-start";
    let call = AdapterCall {
        invocation_ref: ROOT_INVOCATION_REF.into(),
        call_id: "call-atomic-start".into(),
        operation: "cwr.start_action".into(),
        input: json!({
                "activation": "@child--draft--brief",
                "input": {"topic": "launch"}}),
    };
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let messages_before = store
        .conversation(CONVERSATION_REF)
        .unwrap()
        .messages
        .to_vec();
    let activity_before = store
        .execution_conversation(ROOT_INVOCATION_REF)
        .unwrap()
        .activity
        .to_vec();
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, true).unwrap();

    assert!(store
        .start_child_from_host_action_call(
            &call,
            ConversationHostActionCall {
                conversation_ref: CONVERSATION_REF,
                caller_invocation_ref: ROOT_INVOCATION_REF,
                host_call_id: "call-atomic-start",
                nested_invocation_ref: child_ref,
                caller_actor_ref: "cwp://com.example.fixture/actors/worker",
                target_actor_ref: "cwp://com.example.fixture/actors/child",
                activation: "@child--draft--brief",
                input: json!({"topic": "launch"}),
                text: "@child--draft--brief".into(),
            },
            child_request(child_ref, "call-atomic-start",),
            child_adapter_invocation(child_ref),
        )
        .is_err());

    assert!(store.invocation_record(child_ref).is_none());
    assert_eq!(
        store.conversation(CONVERSATION_REF).unwrap().messages,
        messages_before
    );
    assert_eq!(
        store
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .activity,
        activity_before
    );
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, false).unwrap();
    drop(store);
    assert!(open(&path).invocation_record(child_ref).is_none());
}

fn prepared_root() -> ConversationPreparationOutcome {
    let invocation = root_adapter_invocation();
    let conversation = RetainedConversation {
        owner_context_ref: None,
        conversation_ref: CONVERSATION_REF.into(),
        title: None,
        archived: false,
        sidebar_eligible: None,
        parent_conversation_ref: None,
        participant_actor_refs: vec![
            "cwp://com.example.fixture/actors/requester".into(),
            "cwp://com.example.fixture/actors/worker".into(),
        ],
        messages: vec![ConversationMessage {
            file_refs: Vec::new(),
            progress: None,
            message_ref: SOURCE_MESSAGE_REF.into(),
            author_actor_ref: "cwp://com.example.fixture/actors/requester".into(),
            addressed_actor_refs: vec!["cwp://com.example.fixture/actors/worker".into()],
            addressing_mode: Default::default(),
            text: "Please help".into(),
            visualizations: Vec::new(),
            reply_to_message_ref: None,
            action: None,
        }],
        timeline_connection_ref: None,
        activity: Vec::new(),
    };
    let execution_conversation = RetainedConversation {
        owner_context_ref: None,
        conversation_ref: EXECUTION_REF.into(),
        title: None,
        archived: false,
        sidebar_eligible: None,
        parent_conversation_ref: Some(CONVERSATION_REF.into()),
        participant_actor_refs: vec!["cwp://com.example.fixture/actors/worker".into()],
        messages: Vec::new(),
        timeline_connection_ref: None,
        activity: vec![ConversationActivity {
            activity_ref: "cwp://com.example.runtime/activities/root-start".into(),
            sequence: 1,
            occurred_at_unix_ms: 10,
            event: ConversationActivityEvent::TurnStarted {
                invocation_ref: ROOT_INVOCATION_REF.into(),
                source_message_ref: SOURCE_MESSAGE_REF.into(),
                invocation: Some(serde_json::to_value(&invocation).unwrap()),
            },
        }],
    };
    ConversationPreparationOutcome::New(Box::new(PreparedConversationAdmission {
        record: ConversationAdmissionRecord {
            integration_ref: "cwp://com.example.fixture/integrations/slack".into(),
            provider_event_ref: "event-root".into(),
            ingress_fingerprint: None,
            source_input_fingerprint: None,
            conversation_ref: CONVERSATION_REF.into(),
            source_message_ref: SOURCE_MESSAGE_REF.into(),
            execution_conversation_ref: Some(EXECUTION_REF.into()),
            target_actor_ref: Some("cwp://com.example.fixture/actors/worker".into()),
            actor_integration_ref: Some(
                "cwp://com.example.fixture/actor_integrations/worker".into(),
            ),
            adapter_ref: Some("cwp://org.coworkerprotocol/2026-09/adapters/codex".into()),
            invocation_ref: Some(ROOT_INVOCATION_REF.into()),
            state: ConversationAdmissionState::Admitted,
            reply_recipient_visibility: ConversationRecipientVisibility::Implicit,
            reply_message_ref: None,
        },
        conversation,
        execution_conversation: Some(execution_conversation),
        invocation: Some(invocation),
    }))
}

fn prepared_direct_action_root() -> ConversationPreparationOutcome {
    let ConversationPreparationOutcome::New(mut prepared) = prepared_root() else {
        unreachable!("root fixture is always new")
    };
    let invocation = AdapterInvocation::new(
        ROOT_INVOCATION_REF,
        "draft--brief",
        json!({"topic": "launch"}),
        json!({"type": "object"}),
    );
    prepared.invocation = Some(invocation.clone());
    prepared.conversation.messages[0].action = Some(ConversationActionEvent::Call {
        activation: "@worker--draft--brief".into(),
        input: json!({"topic": "launch"}),
    });
    let ConversationActivityEvent::TurnStarted {
        invocation: snapshot,
        ..
    } = &mut prepared.execution_conversation.as_mut().unwrap().activity[0].event
    else {
        unreachable!("root fixture retains its turn start")
    };
    *snapshot = Some(serde_json::to_value(invocation).unwrap());
    ConversationPreparationOutcome::New(prepared)
}

fn child_request(invocation_ref: &str, call_id: &str) -> StartActionInvocation {
    StartActionInvocation {
        invocation_ref: invocation_ref.into(),
        origin: InvocationOrigin::HostCall {
            caller_invocation_ref: ROOT_INVOCATION_REF.into(),
            call_id: call_id.into(),
        },
        requester_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
        target_actor_ref: "cwp://com.example.fixture/actors/child".into(),
        conversation_ref: Some(CONVERSATION_REF.into()),
        actor_integration_ref: Some("cwp://com.example.fixture/actor_integrations/child".into()),
        action_name: "draft--brief".into(),
        input: json!({"topic": "launch"}),
        parent_invocation_ref: Some(ROOT_INVOCATION_REF.into()),
        admitted_at_unix_ms: 20,
    }
}

fn child_adapter_invocation(invocation_ref: &str) -> AdapterInvocation {
    let mut invocation = AdapterInvocation::new(
        invocation_ref,
        "draft--brief",
        json!({"topic": "launch"}),
        json!({"type": "object"}),
    );
    invocation.integration_ref = Some("cwp://com.example.fixture/integrations/codex".into());
    invocation
}

fn commentary(
    model_run_ref: &str,
    response_or_turn_id: &str,
    item_id: &str,
    text: &str,
) -> AdapterCompletedCommentary {
    AdapterCompletedCommentary {
        model_run_ref: model_run_ref.into(),
        provider: "codex".into(),
        thread_id: Some("codex-thread".into()),
        response_or_turn_id: response_or_turn_id.into(),
        item_id: item_id.into(),
        text: text.into(),
    }
}

fn start_claimed_child(
    store: &mut ConversationAdmissionStore,
    child_ref: &str,
    call_id: &str,
    integration_ref: &str,
    started_at_unix_ms: u64,
) -> ClaimedConversationInvocation {
    let call = store
        .append_action_call(
            CONVERSATION_REF,
            "cwp://com.example.fixture/actors/worker",
            "cwp://com.example.fixture/actors/child",
            "@child--draft--brief",
            json!({"topic": call_id}),
            "@child--draft--brief".into(),
        )
        .unwrap();
    let mut adapter_invocation = child_adapter_invocation(child_ref);
    adapter_invocation.integration_ref = Some(integration_ref.into());
    store
        .start_child_invocation(
            child_request(child_ref, call_id),
            &call.message_ref,
            adapter_invocation,
        )
        .unwrap();
    let ConversationInvocationClaimOutcome::Claimed(claimed) = store
        .claim_invocation(child_ref, started_at_unix_ms)
        .unwrap()
    else {
        panic!("child invocation must be claimed")
    };
    *claimed
}
