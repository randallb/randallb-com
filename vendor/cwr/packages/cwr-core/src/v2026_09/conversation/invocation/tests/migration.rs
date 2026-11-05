#[test]
fn fresh_sqlite_state_starts_with_an_empty_invocation_ledger() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");

    let store = open(&path);
    assert!(store.invocation_records().next().is_none());
    assert!(store.runnable_invocation_refs().is_empty());
}

#[test]
fn root_commit_persists_its_invocation_and_execution_mapping_together() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);

    let ConversationAdmissionOutcome::Started(turn) = store.commit(prepared_root()).unwrap() else {
        panic!("root admission must start")
    };
    let invocation = store
        .invocation_record(ROOT_INVOCATION_REF)
        .unwrap()
        .clone();
    assert_eq!(invocation.state, ActionInvocationState::Admitted);
    assert_eq!(
        invocation.origin,
        InvocationOrigin::Ingress {
            integration_ref: "cwp://com.example.fixture/integrations/slack".into(),
            provider_event_ref: "event-root".into(),
        }
    );
    assert_eq!(
        store.execution_conversation_ref(ROOT_INVOCATION_REF),
        Some(EXECUTION_REF)
    );
    assert_eq!(
        turn.record.invocation_ref.as_deref(),
        Some(ROOT_INVOCATION_REF)
    );
    drop(store);

    let reopened = open(&path);
    assert_eq!(
        reopened.invocation_record(ROOT_INVOCATION_REF),
        Some(&invocation)
    );
    assert_eq!(
        reopened
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .conversation_ref,
        EXECUTION_REF
    );
}

#[test]
fn failed_root_commit_rolls_back_the_staged_conversation_and_invocation() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, true).unwrap();

    assert!(store.commit(prepared_root()).is_err());
    assert!(store
        .admission("cwp://com.example.fixture/integrations/slack", "event-root")
        .is_none());
    assert!(store.conversation(CONVERSATION_REF).is_none());
    assert!(store.invocation_record(ROOT_INVOCATION_REF).is_none());
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, false).unwrap();
}

#[test]
fn claim_progress_and_finish_survive_disk_reopen() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();

    let ConversationInvocationClaimOutcome::Claimed(claimed) =
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap()
    else {
        panic!("root invocation must be claimed")
    };
    assert_eq!(claimed.adapter_invocation.continuation, None);
    store
        .record_invocation_progress(ROOT_INVOCATION_REF, "Working")
        .unwrap();
    drop(store);

    let mut reopened = open(&path);
    let running = reopened.invocation_record(ROOT_INVOCATION_REF).unwrap();
    assert_eq!(running.state, ActionInvocationState::Running);
    assert!(running.attempt_active);
    assert_eq!(running.responses[0].status, ActionResponseStatus::PROGRESS);
    let FinishInvocationOutcome::Accepted { response, .. } = reopened
        .finish_invocation(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Done".into(),
                result: Some(json!({"answer": 42})),
                error: None,
            },
            30,
        )
        .unwrap()
    else {
        panic!("finish must be accepted")
    };
    assert_eq!(response.status, ActionResponseStatus::COMPLETED);
    drop(reopened);

    let mut reopened = open(&path);
    let terminal = reopened
        .invocation_record(ROOT_INVOCATION_REF)
        .unwrap()
        .clone();
    assert_eq!(terminal.state, ActionInvocationState::Completed);
    assert!(!terminal.attempt_active);
    assert_eq!(terminal.responses.len(), 2);
    assert_eq!(
        reopened
            .admission_record_for_invocation(ROOT_INVOCATION_REF)
            .unwrap()
            .state,
        ConversationAdmissionState::Completed
    );
    assert!(reopened
        .execution_conversation(ROOT_INVOCATION_REF)
        .unwrap()
        .activity
        .iter()
        .any(|activity| matches!(
            activity.event,
            ConversationActivityEvent::TurnCompleted {
                status: ActionResponseStatus::COMPLETED,
                ..
            }
        )));

    let admission = reopened
        .admission_record_for_invocation(ROOT_INVOCATION_REF)
        .unwrap()
        .clone();
    let admission_index = reopened
        .state
        .admissions
        .iter()
        .position(|record| record.invocation_ref.as_deref() == Some(ROOT_INVOCATION_REF))
        .unwrap();
    reopened.state.admissions[admission_index].state = ConversationAdmissionState::Admitted;
    assert!(reopened.resume_admitted(&admission).unwrap().is_none());
}

#[test]
fn completed_commentary_is_idempotent_and_separate_from_action_progress() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    let messages_before = store
        .conversation(CONVERSATION_REF)
        .unwrap()
        .messages
        .to_vec();
    let ConversationInvocationClaimOutcome::Claimed(claimed) =
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap()
    else {
        panic!("root invocation must be claimed")
    };
    let attempt_count = claimed.claim.invocation.attempt_count;
    let model_run_ref = claimed.model_run_ref.clone();
    assert_eq!(
        claimed.adapter_invocation.model_run_ref.as_deref(),
        Some(model_run_ref.as_str())
    );

    for (item_id, text) in [
        ("message-1", "First update"),
        ("message-2", "Second update"),
    ] {
        assert!(matches!(
            store
                .record_commentary_progress(
                    ROOT_INVOCATION_REF,
                    attempt_count,
                    commentary(&model_run_ref, "turn-1", item_id, text),
                )
                .unwrap(),
            RecordCommentaryProgressOutcome::Recorded(_)
        ));
    }
    assert!(store
        .invocation_record(ROOT_INVOCATION_REF)
        .unwrap()
        .responses
        .is_empty());
    assert_eq!(
        store.conversation(CONVERSATION_REF).unwrap().messages[..messages_before.len()],
        messages_before
    );
    drop(store);

    let mut reopened = open(&path);
    assert!(matches!(
        reopened
            .record_commentary_progress(
                ROOT_INVOCATION_REF,
                attempt_count,
                commentary(&model_run_ref, "turn-1", "message-1", "First update"),
            )
            .unwrap(),
        RecordCommentaryProgressOutcome::Existing(_)
    ));
    assert!(reopened
        .record_commentary_progress(
            ROOT_INVOCATION_REF,
            attempt_count,
            commentary(&model_run_ref, "turn-1", "message-1", "Changed update"),
        )
        .is_err());
    let mut changed_provider = commentary(&model_run_ref, "turn-1", "message-1", "First update");
    changed_provider.provider = "different-provider".into();
    assert!(reopened
        .record_commentary_progress(ROOT_INVOCATION_REF, attempt_count, changed_provider)
        .is_err());
    assert!(matches!(
        reopened
            .record_commentary_progress(
                ROOT_INVOCATION_REF,
                attempt_count,
                commentary(&model_run_ref, "turn-2", "message-1", "Third update"),
            )
            .unwrap(),
        RecordCommentaryProgressOutcome::Recorded(_)
    ));
    assert!(matches!(
        reopened
            .record_commentary_progress(
                ROOT_INVOCATION_REF,
                attempt_count,
                commentary(&model_run_ref, "turn-2", "message-3", "First update"),
            )
            .unwrap(),
        RecordCommentaryProgressOutcome::Recorded(_)
    ));

    let explicit = reopened
        .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 102, "First update")
        .unwrap();
    assert_eq!(explicit.sequence, 1);
    assert_eq!(explicit.status, ActionResponseStatus::PROGRESS);
    let invocation = reopened.invocation_record(ROOT_INVOCATION_REF).unwrap();
    assert_eq!(invocation.responses, vec![explicit]);
    let execution = reopened
        .execution_conversation(ROOT_INVOCATION_REF)
        .unwrap();
    let explicit_progress = execution
        .activity
        .iter()
        .filter_map(|activity| match &activity.event {
            ConversationActivityEvent::Progress {
                status, summary, ..
            } => Some((status.as_u16(), summary.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(explicit_progress, vec![(102, "First update")]);
    let commentary = execution
        .activity
        .iter()
        .filter_map(|activity| match &activity.event {
            ConversationActivityEvent::CommentaryProgress {
                provider_integration_ref,
                response_or_turn_id,
                item_id,
                text,
                ..
            } => Some((
                provider_integration_ref.as_str(),
                response_or_turn_id.as_str(),
                item_id.as_str(),
                text.as_str(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        commentary,
        vec![
            (
                "cwp://com.example.fixture/integrations/codex",
                "turn-1",
                "message-1",
                "First update"
            ),
            (
                "cwp://com.example.fixture/integrations/codex",
                "turn-1",
                "message-2",
                "Second update"
            ),
            (
                "cwp://com.example.fixture/integrations/codex",
                "turn-2",
                "message-1",
                "Third update"
            ),
            (
                "cwp://com.example.fixture/integrations/codex",
                "turn-2",
                "message-3",
                "First update"
            ),
        ]
    );
}
