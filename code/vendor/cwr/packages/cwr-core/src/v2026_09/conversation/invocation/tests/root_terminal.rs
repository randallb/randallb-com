#[test]
fn reopen_repairs_message_less_non_success_without_synthesizing_a_reply() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
        .finish_invocation(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::new(502).unwrap(),
                summary: "Provider failed".into(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("provider_error")
                        .with_message("The provider request failed."),
                ),
            },
            30,
        )
        .unwrap();
    let mut retained = serde_json::to_value(&store.state).unwrap();
    retained["admissions"][0]["state"] = json!("admitted");
    retained["admissions"][0]
        .as_object_mut()
        .unwrap()
        .remove("reply_message_ref");
    let execution = retained["conversations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|conversation| conversation["conversation_ref"] == EXECUTION_REF)
        .unwrap();
    execution["activity"]
        .as_array_mut()
        .unwrap()
        .retain(|activity| activity["event"]["kind"] == "turn_started");
    let retained = serde_json::from_value(retained).unwrap();
    store.persist_state(&retained).unwrap();
    drop(store);

    let reopened = open(&path);
    let admission = reopened
        .admission_record_for_invocation(ROOT_INVOCATION_REF)
        .unwrap();
    assert_eq!(admission.state, ConversationAdmissionState::Failed);
    assert!(admission.reply_message_ref.is_none());
    assert_eq!(
        reopened
            .conversation(CONVERSATION_REF)
            .unwrap()
            .messages
            .len(),
        1
    );
    assert_eq!(
        reopened
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .activity
            .iter()
            .filter(|activity| matches!(
                activity.event,
                ConversationActivityEvent::TurnFailed { .. }
            ))
            .count(),
        1
    );
}

#[test]
fn reopen_clears_a_dangling_non_success_reply_ref_without_synthesizing_a_reply() {
    const MISSING_REPLY_REF: &str = "cwp://com.example.runtime/messages/missing-failure";

    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
        .finish_invocation(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::new(451).unwrap(),
                summary: "Unavailable for policy reasons".into(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("policy_restriction")
                        .with_message("This action is not available."),
                ),
            },
            30,
        )
        .unwrap();
    let mut retained = serde_json::to_value(&store.state).unwrap();
    retained["admissions"][0]["reply_message_ref"] = json!(MISSING_REPLY_REF);
    let retained = serde_json::from_value(retained).unwrap();
    store.persist_state(&retained).unwrap();
    drop(store);

    let reopened = open(&path);
    let admission = reopened
        .admission_record_for_invocation(ROOT_INVOCATION_REF)
        .unwrap();
    assert_eq!(admission.state, ConversationAdmissionState::Failed);
    assert!(admission.reply_message_ref.is_none());
    assert_eq!(
        reopened
            .conversation(CONVERSATION_REF)
            .unwrap()
            .messages
            .len(),
        1
    );
}

#[test]
fn reopen_preserves_a_legacy_non_success_reply_already_in_history() {
    const LEGACY_REPLY_REF: &str = "cwp://com.example.runtime/messages/legacy-failure";

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
                    ActionResponseDetail::new("provider_error")
                        .with_message("The provider request failed."),
                ),
            },
            30,
        )
        .unwrap();
    let mut retained = serde_json::to_value(&store.state).unwrap();
    retained["admissions"][0]["state"] = json!("admitted");
    retained["admissions"][0]["reply_message_ref"] = json!(LEGACY_REPLY_REF);
    let conversation = retained["conversations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|conversation| conversation["conversation_ref"] == CONVERSATION_REF)
        .unwrap();
    conversation["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "message_ref": LEGACY_REPLY_REF,
            "author_actor_ref": "cwp://com.example.fixture/actors/worker",
            "addressed_actor_refs": ["cwp://com.example.fixture/actors/requester"],
            "text": "The provider failed.",
            "reply_to_message_ref": SOURCE_MESSAGE_REF
        }));
    let retained = serde_json::from_value(retained).unwrap();
    store.persist_state(&retained).unwrap();
    drop(store);

    let reopened = open(&path);
    let admission = reopened
        .admission_record_for_invocation(ROOT_INVOCATION_REF)
        .unwrap();
    assert_eq!(admission.state, ConversationAdmissionState::Failed);
    assert_eq!(
        admission.reply_message_ref.as_deref(),
        Some(LEGACY_REPLY_REF)
    );
    let conversation = reopened.conversation(CONVERSATION_REF).unwrap();
    assert_eq!(conversation.messages.len(), 2);
    assert_eq!(conversation.messages[1].message_ref, LEGACY_REPLY_REF);
    assert_eq!(conversation.messages[1].text, "The provider failed.");
}

#[test]
fn reopen_repairs_a_terminal_ledger_with_a_stale_root_projection() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    let ConversationAdmissionOutcome::Started(turn) = store.commit(prepared_root()).unwrap() else {
        panic!("root admission must start")
    };
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
        .finish_invocation(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Answered".into(),
                result: Some(json!({"text": "Here is the retained answer."})),
                error: None,
            },
            30,
        )
        .unwrap();
    // A pre-reconciliation v1 state could retain the canonical terminal
    // ledger while leaving every root conversation projection behind.
    let mut retained = serde_json::to_value(&store.state).unwrap();
    let admission = retained["admissions"][0].as_object_mut().unwrap();
    admission.insert("state".into(), json!("admitted"));
    admission.remove("reply_message_ref");
    let execution = retained["conversations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|conversation| conversation["conversation_ref"] == EXECUTION_REF)
        .unwrap();
    execution["activity"]
        .as_array_mut()
        .unwrap()
        .retain(|activity| activity["event"]["kind"] == "turn_started");
    let retained = serde_json::from_value(retained).unwrap();
    store.persist_state(&retained).unwrap();
    drop(store);

    let reopened = open(&path);
    let admission = reopened
        .admission_record_for_invocation(ROOT_INVOCATION_REF)
        .unwrap();
    assert_eq!(admission.state, ConversationAdmissionState::Completed);
    let reply_ref = admission
        .reply_message_ref
        .as_deref()
        .expect("reopen repairs the missing root reply");
    let conversation = reopened.conversation(CONVERSATION_REF).unwrap();
    assert_eq!(conversation.messages.len(), 2);
    assert_eq!(conversation.messages[1].message_ref, reply_ref);
    assert_eq!(
        conversation.messages[1].text,
        "Here is the retained answer."
    );
    assert!(reopened.runnable_invocation_refs().is_empty());
    assert!(reopened.resume_admitted(&turn.record).unwrap().is_none());
    assert_eq!(
        reopened
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .activity
            .iter()
            .filter(|activity| matches!(
                activity.event,
                ConversationActivityEvent::TurnCompleted { .. }
            ))
            .count(),
        1
    );
    drop(reopened);

    let reopened_again = open(&path);
    assert_eq!(
        reopened_again
            .conversation(CONVERSATION_REF)
            .unwrap()
            .messages
            .len(),
        2
    );
    assert_eq!(
        reopened_again
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .activity
            .iter()
            .filter(|activity| matches!(
                activity.event,
                ConversationActivityEvent::TurnCompleted { .. }
            ))
            .count(),
        1
    );
}

#[test]
fn reopen_repairs_the_missing_reply_after_a_raw_root_finish() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    let ConversationAdmissionOutcome::Started(turn) = store.commit(prepared_root()).unwrap() else {
        panic!("root admission must start")
    };
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store
        .finish_invocation(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Answered".into(),
                result: Some(json!({"text": "Here is the retained answer."})),
                error: None,
            },
            30,
        )
        .unwrap();
    assert_eq!(
        store
            .admission_record_for_invocation(ROOT_INVOCATION_REF)
            .unwrap()
            .state,
        ConversationAdmissionState::Completed
    );
    assert!(store
        .admission_record_for_invocation(ROOT_INVOCATION_REF)
        .unwrap()
        .reply_message_ref
        .is_none());
    drop(store);

    let reopened = open(&path);
    let admission = reopened
        .admission_record_for_invocation(ROOT_INVOCATION_REF)
        .unwrap();
    let reply_ref = admission
        .reply_message_ref
        .as_deref()
        .expect("reopen repairs the missing root reply");
    assert_eq!(admission.state, ConversationAdmissionState::Completed);
    let conversation = reopened.conversation(CONVERSATION_REF).unwrap();
    assert_eq!(conversation.messages.len(), 2);
    assert_eq!(conversation.messages[1].message_ref, reply_ref);
    assert_eq!(
        conversation.messages[1].text,
        "Here is the retained answer."
    );
    assert!(reopened.runnable_invocation_refs().is_empty());
    assert!(reopened.resume_admitted(&turn.record).unwrap().is_none());
    assert_eq!(
        reopened
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .activity
            .iter()
            .filter(|activity| matches!(
                activity.event,
                ConversationActivityEvent::TurnCompleted { .. }
            ))
            .count(),
        1
    );
    drop(reopened);

    let reopened_again = open(&path);
    assert_eq!(
        reopened_again
            .conversation(CONVERSATION_REF)
            .unwrap()
            .messages
            .len(),
        2
    );
    assert_eq!(
        reopened_again
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .activity
            .iter()
            .filter(|activity| matches!(
                activity.event,
                ConversationActivityEvent::TurnCompleted { .. }
            ))
            .count(),
        1
    );
}

#[test]
fn root_direct_action_completion_publishes_its_action_result_atomically() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_direct_action_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();

    let PublishInvocationTerminalOutcome::Accepted { message, .. } = store
        .publish_root_action_completed(
            ROOT_INVOCATION_REF,
            ActionResponseStatus::COMPLETED,
            json!({"artifact_ref": "cwp://com.example.fixture/artifacts/brief"}),
            "Drafted the brief.".into(),
            "Brief drafted".into(),
            30,
        )
        .unwrap()
    else {
        panic!("direct action completion must be accepted")
    };
    let message = message.expect("direct action completion must publish a result");
    assert_eq!(
        message.action,
        Some(ConversationActionEvent::Result {
            activation: "@worker--draft--brief".into(),
            result: json!({"artifact_ref": "cwp://com.example.fixture/artifacts/brief"}),
            outcome: Some(ConversationActionOutcome::Succeeded),
        })
    );
    drop(store);

    let reopened = open(&path);
    assert_eq!(
        reopened
            .conversation(CONVERSATION_REF)
            .unwrap()
            .messages
            .last(),
        Some(&message)
    );
    assert_eq!(
        reopened
            .admission_record_for_invocation(ROOT_INVOCATION_REF)
            .unwrap()
            .state,
        ConversationAdmissionState::Completed
    );
}

#[test]
fn root_direct_action_failure_keeps_a_structured_result_message() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_direct_action_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();

    let PublishInvocationTerminalOutcome::Accepted {
        response, message, ..
    } = store
        .publish_root_failure(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::new(502).unwrap(),
                summary: "Drafting failed".into(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("provider_error")
                        .with_message("The drafting provider failed."),
                ),
            },
            "Could not draft the brief.".into(),
            30,
        )
        .unwrap()
    else {
        panic!("direct action failure must be accepted")
    };
    let message = message.expect("direct action failure must publish a result");
    assert_eq!(
        message.action,
        Some(ConversationActionEvent::Result {
            activation: "@worker--draft--brief".into(),
            result: serde_json::to_value(&response).unwrap(),
            outcome: Some(ConversationActionOutcome::Failed),
        })
    );
    assert_eq!(
        store
            .admission_record_for_invocation(ROOT_INVOCATION_REF)
            .unwrap()
            .reply_message_ref
            .as_deref(),
        Some(message.message_ref.as_str())
    );
}

#[test]
fn failed_root_publication_rolls_back_every_terminal_projection() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, true).unwrap();

    assert!(store
        .publish_root_failure(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::FAILED,
                summary: "Provider failed".into(),
                result: None,
                error: Some(
                    ActionResponseDetail::new("provider_error").with_message("request failed"),
                ),
            },
            "I could not finish that request.".into(),
            30,
        )
        .is_err());
    assert_eq!(
        store.invocation_record(ROOT_INVOCATION_REF).unwrap().state,
        ActionInvocationState::Running
    );
    assert!(
        store
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .attempt_active
    );
    assert_eq!(
        store
            .admission_record_for_invocation(ROOT_INVOCATION_REF)
            .unwrap()
            .state,
        ConversationAdmissionState::Admitted
    );
    assert_eq!(
        store.conversation(CONVERSATION_REF).unwrap().messages.len(),
        1
    );
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, false).unwrap();
    drop(store);

    let reopened = open(&path);
    assert_eq!(
        reopened
            .invocation_record(ROOT_INVOCATION_REF)
            .unwrap()
            .state,
        ActionInvocationState::Running
    );
    assert_eq!(
        reopened
            .admission_record_for_invocation(ROOT_INVOCATION_REF)
            .unwrap()
            .state,
        ConversationAdmissionState::Admitted
    );
    assert_eq!(
        reopened
            .conversation(CONVERSATION_REF)
            .unwrap()
            .messages
            .len(),
        1
    );
}

#[test]
fn deferred_canonical_reopen_repairs_a_missing_terminal_reply_before_publication() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("deferred-terminal.sqlite");
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        &path, RUNTIME_ROOT, ConversationTimelineRollout::NewConversations,
    ).unwrap();
    let ConversationAdmissionOutcome::Started(_) = store.commit(prepared_root()).unwrap() else {
        panic!("root admission must start")
    };
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    store.finish_invocation(ROOT_INVOCATION_REF, InvocationTerminal {
        status: ActionResponseStatus::COMPLETED, summary: "Answered".into(),
        result: Some(json!({"text":"Retained terminal reply"})), error: None,
    }, 30).unwrap();
    assert!(store.conversation(CONVERSATION_REF).unwrap().timeline_connection_ref.is_some());
    assert!(store.admission_record_for_invocation(ROOT_INVOCATION_REF).unwrap().reply_message_ref.is_none());
    drop(store);
    let reopened = ConversationAdmissionStore::open_with_timeline_rollout_deferred(
        &path, RUNTIME_ROOT, ConversationTimelineRollout::NewConversations,
    ).unwrap();
    let reply = reopened.admission_record_for_invocation(ROOT_INVOCATION_REF).unwrap()
        .reply_message_ref.as_ref().expect("canonical cold recovery must repair the reply");
    let history = reopened.full_conversation(CONVERSATION_REF).unwrap().unwrap();
    assert_eq!(history.messages.iter().filter(|message| &message.message_ref == reply).count(), 1);
    assert_eq!(history.messages.last().unwrap().text, "Retained terminal reply");
    drop(reopened);
    let again = ConversationAdmissionStore::open_with_timeline_rollout_deferred(
        &path, RUNTIME_ROOT, ConversationTimelineRollout::NewConversations,
    ).unwrap();
    assert!(again.state.cold_conversation_refs.contains(CONVERSATION_REF));
    assert_eq!(again.full_conversation(CONVERSATION_REF).unwrap().unwrap().messages.len(), 2);
}
