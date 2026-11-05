#[test]
fn child_completion_publishes_action_result_while_parent_stays_live() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let child_ref = "cwp://com.example.runtime/invocations/child-terminal";
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
            child_request(child_ref, "call-start-terminal"),
            &call.message_ref,
            child_adapter_invocation(child_ref),
        )
        .unwrap();
    store.claim_invocation(child_ref, 21).unwrap();

    let PublishInvocationTerminalOutcome::Accepted {
        message,
        woken_invocation_refs,
        ..
    } = store
        .publish_child_action_terminal(
            child_ref,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Child done".into(),
                result: Some(json!({"artifact_ref": "cwp://com.example.fixture/artifacts/brief"})),
                error: None,
            },
            "Drafted the brief.".into(),
            22,
        )
        .unwrap()
    else {
        panic!("child completion must be accepted")
    };
    let message = message.expect("child completion must publish a result");
    assert!(woken_invocation_refs.is_empty());
    assert!(matches!(
        message.action,
        Some(ConversationActionEvent::Result {
            outcome: Some(ConversationActionOutcome::Succeeded),
            ..
        })
    ));
    drop(store);

    let reopened = open(&path);
    assert_eq!(
        reopened.invocation_record(child_ref).unwrap().state,
        ActionInvocationState::Completed
    );
    assert_eq!(
        reopened
            .conversation(CONVERSATION_REF)
            .unwrap()
            .messages
            .last(),
        Some(&message)
    );
    assert!(reopened.runnable_invocation_refs().is_empty());
    assert!(
        reopened
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .attempt_active
    );
}

#[test]
fn child_failure_keeps_a_structured_result_message() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let child_ref = "cwp://com.example.runtime/invocations/child-failed";
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
            child_request(child_ref, "call-start-failed"),
            &call.message_ref,
            child_adapter_invocation(child_ref),
        )
        .unwrap();
    store.claim_invocation(child_ref, 21).unwrap();

    let PublishInvocationTerminalOutcome::Accepted {
        response, message, ..
    } = store
        .publish_child_action_terminal(
            child_ref,
            InvocationTerminal {
                status: ActionResponseStatus::new(502).unwrap(),
                summary: "Child failed".into(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("provider_error")
                        .with_message("The child provider failed."),
                ),
            },
            "Could not draft the brief.".into(),
            22,
        )
        .unwrap()
    else {
        panic!("child failure must be accepted")
    };
    let message = message.expect("child failure must publish a result");
    assert_eq!(
        message.action,
        Some(ConversationActionEvent::Result {
            activation: "@child--draft--brief".into(),
            result: serde_json::to_value(&response).unwrap(),
            outcome: Some(ConversationActionOutcome::Failed),
        })
    );
}

#[test]
fn stopped_root_is_visibly_terminal_and_never_resumes_after_reopen() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    let ConversationAdmissionOutcome::Started(turn) = store.commit(prepared_root()).unwrap() else {
        panic!("root admission must start")
    };
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();

    let PublishInvocationStopOutcome::Accepted {
        response,
        message,
        stopped_invocation_refs,
        woken_invocation_refs,
    } = store
        .publish_root_stop(
            ROOT_INVOCATION_REF,
            InvocationStopRequest {
                requested_at_unix_ms: 30,
                requested_by_actor_ref: Some("cwp://com.example.fixture/actors/requester".into()),
                request_id: None,
                reason: Some("No longer needed".into()),
            },
            "Stopped that request.".into(),
        )
        .unwrap()
    else {
        panic!("root stop must be accepted")
    };
    assert_eq!(response.status, ActionResponseStatus::STOPPED);
    assert_eq!(response.status.as_u16(), 499);
    assert_eq!(
        response
            .cause
            .as_ref()
            .and_then(|cause| cause.message.as_deref()),
        Some("No longer needed")
    );
    assert_eq!(
        store
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .terminal_response(),
        Some(&response)
    );
    assert_eq!(stopped_invocation_refs, vec![ROOT_INVOCATION_REF]);
    assert!(woken_invocation_refs.is_empty());
    assert!(message.is_none());
    assert_eq!(
        store.conversation(CONVERSATION_REF).unwrap().messages.len(),
        1
    );
    drop(store);

    let reopened = open(&path);
    let admission = reopened
        .admission_record_for_invocation(ROOT_INVOCATION_REF)
        .unwrap();
    assert_eq!(admission.state, ConversationAdmissionState::Stopped);
    assert!(admission.reply_message_ref.is_none());
    assert!(reopened.runnable_invocation_refs().is_empty());
    assert!(reopened.resume_admitted(&turn.record).unwrap().is_none());
    assert!(reopened
        .execution_conversation(ROOT_INVOCATION_REF)
        .unwrap()
        .activity
        .iter()
        .any(|activity| matches!(
            activity.event,
            ConversationActivityEvent::TurnStopped { .. }
        )));
}

#[test]
fn progress_response_and_activity_roll_back_as_one_write() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let initial_activity_count = store
        .execution_conversation(ROOT_INVOCATION_REF)
        .unwrap()
        .activity
        .len();
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, true).unwrap();

    assert!(store
        .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 102, "Working")
        .is_err());
    assert!(store
        .invocation_record(ROOT_INVOCATION_REF)
        .unwrap()
        .responses
        .is_empty());
    assert_eq!(
        store
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .activity
            .len(),
        initial_activity_count
    );
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, false).unwrap();
    drop(store);

    let reopened = open(&path);
    assert!(reopened
        .invocation_record(ROOT_INVOCATION_REF)
        .unwrap()
        .responses
        .is_empty());
    assert_eq!(
        reopened
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .activity
            .len(),
        initial_activity_count
    );
}

#[test]
fn blocked_root_is_published_as_a_failed_admission_with_retained_detail() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();

    let PublishInvocationTerminalOutcome::Accepted {
        response, message, ..
    } = store
        .publish_root_failure(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::BLOCKED,
                summary: "Need approval".into(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("approval_required")
                        .with_message("A reviewer must approve this."),
                ),
            },
            "I need approval before I can continue.".into(),
            30,
        )
        .unwrap()
    else {
        panic!("blocked publication must be accepted")
    };
    assert_eq!(response.status, ActionResponseStatus::BLOCKED);
    assert!(message.is_none());
    assert!(store
        .admission_record_for_invocation(ROOT_INVOCATION_REF)
        .unwrap()
        .reply_message_ref
        .is_none());
    assert_eq!(
        store.conversation(CONVERSATION_REF).unwrap().messages.len(),
        1
    );
    drop(store);

    let reopened = open(&path);
    assert_eq!(
        reopened
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .state,
        ActionInvocationState::Blocked
    );
    assert_eq!(
        reopened
            .admission_record_for_invocation(ROOT_INVOCATION_REF)
            .unwrap()
            .state,
        ConversationAdmissionState::Failed
    );
    assert_eq!(
        reopened
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .terminal_response()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .code,
        "approval_required"
    );
}

#[test]
fn child_stop_publishes_failed_action_result_while_parent_stays_live() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let child_ref = "cwp://com.example.runtime/invocations/child-stopped";
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
            child_request(child_ref, "call-start-stop"),
            &call.message_ref,
            child_adapter_invocation(child_ref),
        )
        .unwrap();

    let PublishInvocationStopOutcome::Accepted {
        message,
        woken_invocation_refs,
        ..
    } = store
        .publish_child_action_stop(
            child_ref,
            InvocationStopRequest {
                requested_at_unix_ms: 30,
                requested_by_actor_ref: Some("cwp://com.example.fixture/actors/worker".into()),
                request_id: None,
                reason: Some("No longer needed".into()),
            },
            "Stopped drafting the brief.".into(),
        )
        .unwrap()
    else {
        panic!("child stop must be accepted")
    };
    let message = message.expect("child stop must publish a result");
    assert!(woken_invocation_refs.is_empty());
    assert!(matches!(
        message.action,
        Some(ConversationActionEvent::Result {
            outcome: Some(ConversationActionOutcome::Failed),
            ..
        })
    ));
    drop(store);

    let reopened = open(&path);
    assert_eq!(
        reopened.invocation_record(child_ref).unwrap().state,
        ActionInvocationState::Stopped
    );
    assert!(reopened.runnable_invocation_refs().is_empty());
    assert!(
        reopened
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .attempt_active
    );
    assert_eq!(
        reopened
            .conversation(CONVERSATION_REF)
            .unwrap()
            .messages
            .last(),
        Some(&message)
    );
}

#[test]
fn published_child_stop_completes_a_matching_raw_stop_without_duplicate_terminal_state() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let child_ref = "cwp://com.example.runtime/invocations/raw-child-stopped";
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
            child_request(child_ref, "call-start-raw-stop"),
            &call.message_ref,
            child_adapter_invocation(child_ref),
        )
        .unwrap();
    let request = InvocationStopRequest {
        requested_at_unix_ms: 30,
        requested_by_actor_ref: Some("cwp://com.example.fixture/actors/worker".into()),
        request_id: Some("event-raw-then-publish-child".into()),
        reason: Some("No longer needed".into()),
    };

    assert!(matches!(
        store.stop_invocation(child_ref, request.clone()).unwrap(),
        StopInvocationOutcome::Accepted {
            replayed: false,
            ..
        }
    ));
    let activity_count = store
        .execution_conversation(child_ref)
        .unwrap()
        .activity
        .len();
    let message_count = store.conversation(CONVERSATION_REF).unwrap().messages.len();

    let PublishInvocationStopOutcome::Accepted {
        response,
        message,
        stopped_invocation_refs,
        ..
    } = store
        .publish_child_action_stop(
            child_ref,
            request.clone(),
            "Stopped drafting the brief.".into(),
        )
        .unwrap()
    else {
        panic!("matching raw child stop must finish publication")
    };
    let message = message.expect("child stop must publish a result");
    assert_eq!(stopped_invocation_refs, [child_ref.to_owned()]);
    assert_eq!(response.status, ActionResponseStatus::STOPPED);
    assert_eq!(message.text, "Stopped drafting the brief.");
    assert_eq!(
        store.invocation_record(child_ref).unwrap().responses.len(),
        1
    );
    assert_eq!(
        store
            .execution_conversation(child_ref)
            .unwrap()
            .activity
            .len(),
        activity_count
    );
    assert_eq!(
        store.conversation(CONVERSATION_REF).unwrap().messages.len(),
        message_count + 1
    );

    let PublishInvocationStopOutcome::Accepted {
        message: replayed_message,
        ..
    } = store
        .publish_child_action_stop(child_ref, request, "Changed replay text".into())
        .unwrap()
    else {
        panic!("published child stop must replay as accepted")
    };
    assert_eq!(replayed_message.as_ref(), Some(&message));
    assert_eq!(
        store.conversation(CONVERSATION_REF).unwrap().messages.len(),
        message_count + 1
    );
}
