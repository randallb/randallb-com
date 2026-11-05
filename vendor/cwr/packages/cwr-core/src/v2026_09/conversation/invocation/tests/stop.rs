#[test]
fn stop_and_attached_propagation_are_persisted_atomically() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let child_ref = "cwp://com.example.runtime/invocations/attached";
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
            child_request(child_ref, "call-start"),
            &call.message_ref,
            child_adapter_invocation(child_ref),
        )
        .unwrap();

    let StopInvocationOutcome::Accepted {
        stopped_invocation_refs,
        ..
    } = store
        .stop_invocation(
            ROOT_INVOCATION_REF,
            InvocationStopRequest {
                requested_at_unix_ms: 30,
                requested_by_actor_ref: Some("cwp://com.example.fixture/actors/requester".into()),
                request_id: None,
                reason: Some("No longer needed".into()),
            },
        )
        .unwrap()
    else {
        panic!("stop must be accepted")
    };
    assert_eq!(
        stopped_invocation_refs,
        vec![ROOT_INVOCATION_REF.to_owned(), child_ref.to_owned()]
    );
    drop(store);

    let reopened = open(&path);
    for invocation_ref in [ROOT_INVOCATION_REF, child_ref] {
        assert_eq!(
            reopened.invocation_record(invocation_ref).unwrap().state,
            if invocation_ref == ROOT_INVOCATION_REF {
                ActionInvocationState::Stopping
            } else {
                ActionInvocationState::Stopped
            }
        );
        assert!(reopened
            .execution_conversation(invocation_ref)
            .unwrap()
            .activity
            .iter()
            .any(|activity| matches!(
                activity.event,
                ConversationActivityEvent::TurnStopped { .. }
            )));
    }
    assert_eq!(
        reopened
            .admission_record_for_invocation(ROOT_INVOCATION_REF)
            .unwrap()
            .state,
        ConversationAdmissionState::Stopped
    );
}

#[test]
fn identical_published_root_stop_replays_as_accepted_without_another_message() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let request = InvocationStopRequest {
        requested_at_unix_ms: 30,
        requested_by_actor_ref: Some("cwp://com.example.fixture/actors/requester".into()),
        request_id: Some("event-stop-root".into()),
        reason: None,
    };

    let PublishInvocationStopOutcome::Accepted {
        response,
        message,
        stopped_invocation_refs,
        ..
    } = store
        .publish_root_stop(
            ROOT_INVOCATION_REF,
            request.clone(),
            "Stopped that request.".into(),
        )
        .unwrap()
    else {
        panic!("first root stop must be accepted")
    };
    let message_count = store.conversation(CONVERSATION_REF).unwrap().messages.len();

    let PublishInvocationStopOutcome::Accepted {
        response: replayed_response,
        message: replayed_message,
        stopped_invocation_refs: replayed_refs,
        ..
    } = store
        .publish_root_stop(ROOT_INVOCATION_REF, request, "Changed replay text".into())
        .unwrap()
    else {
        panic!("identical root stop must replay as accepted")
    };
    assert_eq!(replayed_response, response);
    assert!(message.is_none());
    assert_eq!(replayed_message, message);
    assert_eq!(replayed_refs, stopped_invocation_refs);
    assert_eq!(
        store.conversation(CONVERSATION_REF).unwrap().messages.len(),
        message_count
    );
}

#[test]
fn identical_raw_stop_replay_does_not_require_another_state_write() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let request = InvocationStopRequest {
        requested_at_unix_ms: 30,
        requested_by_actor_ref: Some("cwp://com.example.fixture/actors/requester".into()),
        request_id: Some("event-stop-root".into()),
        reason: Some("No longer needed".into()),
    };

    assert!(matches!(
        store
            .stop_invocation(ROOT_INVOCATION_REF, request.clone())
            .unwrap(),
        StopInvocationOutcome::Accepted {
            replayed: false,
            ..
        }
    ));

    let blocked_parent = temporary.path().join("blocked-parent");
    fs::write(&blocked_parent, b"not a directory").unwrap();
    store.state_path = blocked_parent.join("conversation.json");
    assert!(matches!(
        store.stop_invocation(ROOT_INVOCATION_REF, request).unwrap(),
        StopInvocationOutcome::Accepted { replayed: true, .. }
    ));
}

#[test]
fn published_root_stop_completes_a_matching_raw_stop_without_duplicate_terminal_state() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let child_ref = "cwp://com.example.runtime/invocations/raw-attached-child-stopped";
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
            child_request(child_ref, "call-start-raw-attached-stop"),
            &call.message_ref,
            child_adapter_invocation(child_ref),
        )
        .unwrap();
    let request = InvocationStopRequest {
        requested_at_unix_ms: 30,
        requested_by_actor_ref: Some("cwp://com.example.fixture/actors/requester".into()),
        request_id: Some("event-raw-then-publish-root".into()),
        reason: Some("No longer needed".into()),
    };

    assert!(matches!(
        store
            .stop_invocation(ROOT_INVOCATION_REF, request.clone())
            .unwrap(),
        StopInvocationOutcome::Accepted {
            replayed: false,
            ..
        }
    ));
    let activity_counts = [ROOT_INVOCATION_REF, child_ref].map(|invocation_ref| {
        store
            .execution_conversation(invocation_ref)
            .unwrap()
            .activity
            .len()
    });
    let message_count = store.conversation(CONVERSATION_REF).unwrap().messages.len();

    let PublishInvocationStopOutcome::Accepted {
        response,
        message,
        stopped_invocation_refs,
        ..
    } = store
        .publish_root_stop(
            ROOT_INVOCATION_REF,
            request.clone(),
            "Stopped that request.".into(),
        )
        .unwrap()
    else {
        panic!("matching raw root stop must finish publication")
    };
    assert_eq!(
        stopped_invocation_refs,
        [ROOT_INVOCATION_REF.to_owned(), child_ref.to_owned()]
    );
    assert_eq!(response.status, ActionResponseStatus::STOPPED);
    assert!(message.is_none());
    for (invocation_ref, activity_count) in [ROOT_INVOCATION_REF, child_ref]
        .into_iter()
        .zip(activity_counts)
    {
        assert_eq!(
            store
                .invocation_record(invocation_ref)
                .unwrap()
                .responses
                .len(),
            1
        );
        assert_eq!(
            store
                .execution_conversation(invocation_ref)
                .unwrap()
                .activity
                .len(),
            activity_count
        );
    }
    assert_eq!(
        store.conversation(CONVERSATION_REF).unwrap().messages.len(),
        message_count + 1
    );

    let PublishInvocationStopOutcome::Accepted {
        message: replayed_message,
        ..
    } = store
        .publish_root_stop(ROOT_INVOCATION_REF, request, "Changed replay text".into())
        .unwrap()
    else {
        panic!("published root stop must replay as accepted")
    };
    assert_eq!(replayed_message, message);
    assert_eq!(
        store.conversation(CONVERSATION_REF).unwrap().messages.len(),
        message_count + 1
    );
}

#[test]
fn stop_selection_is_conversation_scoped_and_never_guesses_between_active_roots() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();

    let selected = store
        .select_invocation_for_stop(Some(CONVERSATION_REF), None)
        .unwrap();
    assert_eq!(selected_ref(selected), Some(ROOT_INVOCATION_REF.into()));

    store
        .state
        .invocation_ledger
        .claim(ROOT_INVOCATION_REF, 11)
        .unwrap();
    let awaited_child_ref = "cwp://com.example.runtime/invocations/waited-child";
    store
        .state
        .invocation_ledger
        .start(selector_child(awaited_child_ref))
        .unwrap();
    let queued_ref = "cwp://com.example.runtime/invocations/a-queued";
    store
        .state
        .invocation_ledger
        .start(selector_root(queued_ref, CONVERSATION_REF, 20))
        .unwrap();
    let selected = store
        .select_invocation_for_stop(Some(CONVERSATION_REF), None)
        .unwrap();
    assert_eq!(selected_ref(selected), Some(ROOT_INVOCATION_REF.into()));

    store.state.invocation_ledger.claim(queued_ref, 21).unwrap();
    let InvocationStopSelection::Ambiguous(records) = store
        .select_invocation_for_stop(Some(CONVERSATION_REF), None)
        .unwrap()
    else {
        panic!("two active roots must be ambiguous")
    };
    assert_eq!(
        records.iter().map(String::as_str).collect::<Vec<_>>(),
        [ROOT_INVOCATION_REF, queued_ref]
    );

    assert_eq!(
        selected_ref(
            store
                .select_invocation_for_stop(Some(CONVERSATION_REF), Some("a-queued"))
                .unwrap()
        ),
        Some(queued_ref.into())
    );
    assert!(matches!(
        store
            .select_invocation_for_stop(
                Some("cwp://com.example.runtime/conversations/elsewhere"),
                Some(queued_ref),
            )
            .unwrap(),
        InvocationStopSelection::None
    ));
}

#[test]
fn implicit_stop_selection_ignores_children_and_terminal_roots() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store
        .state
        .invocation_ledger
        .claim(ROOT_INVOCATION_REF, 11)
        .unwrap();
    let child_ref = "cwp://com.example.runtime/invocations/child-selector";
    store
        .state
        .invocation_ledger
        .start(selector_child(child_ref))
        .unwrap();
    store
        .state
        .invocation_ledger
        .finish(
            ROOT_INVOCATION_REF,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Done".into(),
                result: Some(json!({"ok": true})),
                error: None,
            },
            30,
        )
        .unwrap();

    assert!(matches!(
        store
            .select_invocation_for_stop(Some(CONVERSATION_REF), None)
            .unwrap(),
        InvocationStopSelection::None
    ));
    assert_eq!(
        selected_ref(
            store
                .select_invocation_for_stop(Some(CONVERSATION_REF), Some(ROOT_INVOCATION_REF))
                .unwrap()
        ),
        Some(ROOT_INVOCATION_REF.into()),
        "explicit terminal selection must survive so the stop can return too-late"
    );
    assert_eq!(
        selected_ref(
            store
                .select_invocation_for_stop(Some(CONVERSATION_REF), Some("child-selector"))
                .unwrap()
        ),
        Some(child_ref.into())
    );
    assert!(store
        .select_invocation_for_stop(None, None)
        .unwrap_err()
        .to_string()
        .contains("conversation"));
}

#[test]
fn stop_selection_reports_queued_and_short_id_ambiguity() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    let first_ref = "cwp://com.example.runtime/branches/one/invocations/shared";
    let second_ref = "cwp://com.example.runtime/branches/two/invocations/shared";
    store
        .state
        .invocation_ledger
        .start(selector_root(first_ref, CONVERSATION_REF, 10))
        .unwrap();
    store
        .state
        .invocation_ledger
        .start(selector_root(second_ref, CONVERSATION_REF, 20))
        .unwrap();

    let InvocationStopSelection::Ambiguous(queued) = store
        .select_invocation_for_stop(Some(CONVERSATION_REF), None)
        .unwrap()
    else {
        panic!("two queued roots must be ambiguous")
    };
    assert_eq!(
        queued.iter().map(String::as_str).collect::<Vec<_>>(),
        [first_ref, second_ref]
    );
    assert!(matches!(
        store
            .select_invocation_for_stop(Some(CONVERSATION_REF), Some("shared"))
            .unwrap(),
        InvocationStopSelection::Ambiguous(records) if records.len() == 2
    ));
    assert_eq!(
        selected_ref(
            store
                .select_invocation_for_stop(Some(CONVERSATION_REF), Some(first_ref))
                .unwrap()
        ),
        Some(first_ref.into())
    );
}

#[test]
fn stop_authority_is_direct_or_inherited_from_the_ingress_requester() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store
        .state
        .invocation_ledger
        .claim(ROOT_INVOCATION_REF, 11)
        .unwrap();
    let child_ref = "cwp://com.example.runtime/invocations/detached-authority";
    store
        .state
        .invocation_ledger
        .start(selector_child(child_ref))
        .unwrap();

    assert!(store
        .is_actor_authorized_to_stop_invocation(
            ROOT_INVOCATION_REF,
            "cwp://com.example.fixture/actors/requester"
        )
        .unwrap());
    assert!(store
        .is_actor_authorized_to_stop_invocation(
            ROOT_INVOCATION_REF,
            "cwp://com.example.fixture/actors/worker"
        )
        .unwrap());
    assert!(store
        .is_actor_authorized_to_stop_invocation(child_ref, "cwp://com.example.fixture/actors/child")
        .unwrap());
    assert!(store
        .is_actor_authorized_to_stop_invocation(
            child_ref,
            "cwp://com.example.fixture/actors/requester"
        )
        .unwrap());
    assert!(!store
        .is_actor_authorized_to_stop_invocation(
            child_ref,
            "cwp://com.example.fixture/actors/observer"
        )
        .unwrap());
}

fn selected_ref(selection: InvocationStopSelection) -> Option<String> {
    match selection {
        InvocationStopSelection::Selected(invocation_ref) => Some(invocation_ref),
        InvocationStopSelection::None | InvocationStopSelection::Ambiguous(_) => None,
    }
}

fn selector_root(
    invocation_ref: &str,
    conversation_ref: &str,
    admitted_at_unix_ms: u64,
) -> StartActionInvocation {
    StartActionInvocation {
        invocation_ref: invocation_ref.into(),
        origin: InvocationOrigin::Ingress {
            integration_ref: "cwp://com.example.fixture/integrations/slack".into(),
            provider_event_ref: format!("event-{invocation_ref}"),
        },
        requester_actor_ref: "cwp://com.example.fixture/actors/requester".into(),
        target_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
        conversation_ref: Some(conversation_ref.into()),
        actor_integration_ref: Some("cwp://com.example.fixture/actor_integrations/worker".into()),
        action_name: RESPOND_TO_MESSAGE_ACTION_NAME.into(),
        input: json!({}),
        parent_invocation_ref: None,
        admitted_at_unix_ms,
    }
}

fn selector_child(invocation_ref: &str) -> StartActionInvocation {
    StartActionInvocation {
        invocation_ref: invocation_ref.into(),
        origin: InvocationOrigin::HostCall {
            caller_invocation_ref: ROOT_INVOCATION_REF.into(),
            call_id: format!("call-{invocation_ref}"),
        },
        requester_actor_ref: "cwp://com.example.fixture/actors/worker".into(),
        target_actor_ref: "cwp://com.example.fixture/actors/child".into(),
        conversation_ref: Some(CONVERSATION_REF.into()),
        actor_integration_ref: Some("cwp://com.example.fixture/actor_integrations/child".into()),
        action_name: "draft--brief".into(),
        input: json!({}),
        parent_invocation_ref: Some(ROOT_INVOCATION_REF.into()),
        admitted_at_unix_ms: 20,
    }
}
