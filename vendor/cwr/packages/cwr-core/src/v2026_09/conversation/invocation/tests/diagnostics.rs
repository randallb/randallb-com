#[test]
fn failed_invocation_mutation_rolls_back_in_memory() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("state");
    fs::create_dir(&directory).unwrap();
    let path = directory.join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, true).unwrap();

    assert!(store.claim_invocation(ROOT_INVOCATION_REF, 11).is_err());
    let invocation = store.invocation_record(ROOT_INVOCATION_REF).unwrap();
    assert_eq!(invocation.state, ActionInvocationState::Admitted);
    assert!(!invocation.attempt_active);
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, false).unwrap();
}

#[test]
fn failed_terminal_detail_is_persisted() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
        .finish_invocation(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::FAILED,
                summary: "Provider failed".into(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("provider_error").with_message("request failed"),
                ),
            },
            30,
        )
        .unwrap();
    drop(store);

    let reopened = open(&path);
    let terminal = reopened
        .invocation_record(ROOT_INVOCATION_REF)
        .unwrap()
        .terminal_response()
        .unwrap();
    assert_eq!(terminal.status, ActionResponseStatus::FAILED);
    assert_eq!(terminal.error.as_ref().unwrap().code, "provider_error");
}

#[test]
fn invocation_diagnostics_preserve_canonical_failure_statuses() {
    for (case, terminal, expected_status) in [
        (
            "blocked",
            InvocationTerminal {
                status: ActionResponseStatus::BLOCKED,
                summary: "Need approval".into(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("approval_required")
                        .with_message("A reviewer must approve this."),
                ),
            },
            422,
        ),
        (
            "failed",
            InvocationTerminal {
                status: ActionResponseStatus::FAILED,
                summary: "Provider failed".into(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("provider_error")
                        .with_message("The provider request failed."),
                ),
            },
            500,
        ),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join(format!("conversation-{case}.json"));
        let mut store = open(&path);
        store.commit(prepared_root()).unwrap();
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
        store
            .publish_root_failure(
                ROOT_INVOCATION_REF,
                terminal,
                "The invocation could not complete.".into(),
                30,
            )
            .unwrap();

        let report = store.inspect_invocation(ROOT_INVOCATION_REF).unwrap();
        assert_eq!(
            report
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.status),
            Some(expected_status),
            "{case} diagnostics lost the canonical failure status"
        );
    }
}

#[test]
fn latest_invocation_diagnostics_use_the_canonical_execution_mapping() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let stale_execution_ref = "cwp://com.example.runtime/conversations/stale-root-execution";
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
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
            "The invocation could not complete.".into(),
            30,
        )
        .unwrap();

    let mut staged = store.state.clone();
    staged
        .admissions
        .iter_mut()
        .find(|record| record.invocation_ref.as_deref() == Some(ROOT_INVOCATION_REF))
        .unwrap()
        .execution_conversation_ref = Some(stale_execution_ref.into());
    staged.conversations.push(RetainedConversation {
        owner_context_ref: None,
        conversation_ref: stale_execution_ref.into(),
        title: None,
        archived: false,
        sidebar_eligible: None,
        parent_conversation_ref: Some(CONVERSATION_REF.into()),
        participant_actor_refs: vec!["cwp://com.example.fixture/actors/worker".into()],
        messages: Vec::new(),
        timeline_connection_ref: None,
        activity: vec![ConversationActivity {
            activity_ref: "cwp://com.example.runtime/activities/stale-terminal".into(),
            sequence: 1,
            occurred_at_unix_ms: 30,
            event: ConversationActivityEvent::TurnCompleted {
                status: ActionResponseStatus::new(204).unwrap(),
                summary: "Stale terminal".into(),
                result: None,
            },
        }],
    });
    super::validate_invocation_execution_links(&staged)
        .expect("canonical execution mapping makes the retained state valid");
    store.replace_state(staged).unwrap();
    drop(store);

    let reopened = open(&path);
    let selected = reopened.inspect_invocation(ROOT_INVOCATION_REF).unwrap();
    let latest = reopened
        .inspect_latest_invocation_for_conversation(CONVERSATION_REF)
        .unwrap();
    assert_eq!(latest, selected);
    assert_eq!(latest.execution_conversation_ref, EXECUTION_REF);
}

#[test]
fn root_message_completion_publishes_one_recoverable_terminal_state() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let progress = store
        .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 102, "Working")
        .unwrap();
    assert_eq!(progress.status, ActionResponseStatus::PROGRESS);

    let PublishInvocationTerminalOutcome::Accepted {
        response,
        message,
        woken_invocation_refs,
    } = store
        .publish_root_message_completed(
            ROOT_INVOCATION_REF,
            ActionResponseStatus::COMPLETED,
            json!({"text": "Here is the answer."}),
            "Answered".into(),
            30,
        )
        .unwrap()
    else {
        panic!("root completion must be accepted")
    };
    assert_eq!(response.status, ActionResponseStatus::COMPLETED);
    let message = message.expect("completed message action must publish a reply");
    assert_eq!(message.text, "Here is the answer.");
    assert!(woken_invocation_refs.is_empty());
    drop(store);

    let reopened = open(&path);
    assert_eq!(
        reopened
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );
    let admission = reopened
        .admission_record_for_invocation(ROOT_INVOCATION_REF)
        .unwrap();
    assert_eq!(admission.state, ConversationAdmissionState::Completed);
    assert_eq!(
        admission.reply_message_ref.as_deref(),
        Some(message.message_ref.as_str())
    );
    assert!(reopened
        .execution_conversation(ROOT_INVOCATION_REF)
        .unwrap()
        .activity
        .iter()
        .any(|activity| matches!(
            &activity.event,
            ConversationActivityEvent::Progress {
                status: ActionResponseStatus::PROGRESS,
                summary, ..
            } if summary == "Working"
        )));
    assert!(reopened
        .execution_conversation(ROOT_INVOCATION_REF)
        .unwrap()
        .activity
        .iter()
        .any(|activity| matches!(
            &activity.event,
            ConversationActivityEvent::TurnCompleted {
                status: ActionResponseStatus::COMPLETED,
                summary,
                ..
            } if summary == "Answered"
        )));
}

#[test]
fn root_message_blocked_and_failed_results_are_retained_without_replies() {
    for (case, terminal) in [
        (
            "blocked",
            InvocationTerminal {
                status: ActionResponseStatus::new(451).unwrap(),
                summary: "Waiting for approval".into(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("approval_required")
                        .with_message("A reviewer must approve this."),
                ),
            },
        ),
        (
            "failed",
            InvocationTerminal {
                status: ActionResponseStatus::new(503).unwrap(),
                summary: "Provider unavailable".into(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("provider_unavailable")
                        .with_message("The provider could not be reached."),
                ),
            },
        ),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join(format!("conversation-{case}.json"));
        let mut store = open(&path);
        store.commit(prepared_root()).unwrap();
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
        let initial_messages = store.conversation(CONVERSATION_REF).unwrap().messages.len();

        let PublishInvocationTerminalOutcome::Accepted {
            response, message, ..
        } = store
            .publish_root_failure(
                ROOT_INVOCATION_REF,
                terminal,
                "This text must not become coworker speech.".into(),
                30,
            )
            .unwrap()
        else {
            panic!("{case} root result must be accepted")
        };
        assert!(message.is_none(), "{case} created a participant reply");
        assert_eq!(
            store.conversation(CONVERSATION_REF).unwrap().messages.len(),
            initial_messages,
            "{case} changed the participant message history"
        );
        let admission = store
            .admission_record_for_invocation(ROOT_INVOCATION_REF)
            .unwrap();
        assert_eq!(admission.state, ConversationAdmissionState::Failed);
        assert!(admission.reply_message_ref.is_none());
        assert!(
            store
                .execution_conversation(ROOT_INVOCATION_REF)
                .unwrap()
                .activity
                .iter()
                .any(|activity| matches!(
                    activity.event,
                    ConversationActivityEvent::TurnFailed { status, .. }
                        if status == response.status
                )),
            "{case} did not record the matching terminal activity"
        );
        assert_eq!(
            store
                .invocation_record(ROOT_INVOCATION_REF)
                .unwrap()
                .terminal_response(),
            Some(&response),
            "{case} did not retain the exact action response"
        );
        drop(store);

        let reopened = open(&path);
        assert_eq!(
            reopened
                .invocation_record(ROOT_INVOCATION_REF)
                .unwrap()
                .terminal_response(),
            Some(&response)
        );
        assert!(reopened
            .admission_record_for_invocation(ROOT_INVOCATION_REF)
            .unwrap()
            .reply_message_ref
            .is_none());
        assert_eq!(
            reopened
                .conversation(CONVERSATION_REF)
                .unwrap()
                .messages
                .len(),
            initial_messages
        );
    }
}
