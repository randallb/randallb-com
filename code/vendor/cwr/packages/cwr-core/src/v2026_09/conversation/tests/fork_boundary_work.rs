#[test]
fn later_message_fork_preserves_failed_and_stopped_work_without_replies() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("terminal-without-reply.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();

    let failed = admit_context_turn(&mut store, &graph, None, "failed-root", "Fail this work");
    let conversation_ref = failed.conversation.conversation_ref;
    let failed_invocation_ref = failed.record.invocation_ref.as_deref().unwrap().to_owned();
    assert!(matches!(
        store.claim_invocation(&failed_invocation_ref, 100).unwrap(),
        ConversationInvocationClaimOutcome::Claimed(_)
    ));
    assert!(store
        .fail(&failed_invocation_ref, "Expected failure".into())
        .unwrap()
        .is_none());

    let stopped = admit_context_turn(
        &mut store,
        &graph,
        Some(&conversation_ref),
        "stopped-root",
        "Stop this work",
    );
    let stopped_invocation_ref = stopped.record.invocation_ref.as_deref().unwrap().to_owned();
    assert!(matches!(
        store.claim_invocation(&stopped_invocation_ref, 101).unwrap(),
        ConversationInvocationClaimOutcome::Claimed(_)
    ));
    store
        .stop_invocation(
            &stopped_invocation_ref,
            crate::v2026_09::invocation::InvocationStopRequest::new(102),
        )
        .unwrap();
    store
        .settle_stopped_invocation(&stopped_invocation_ref, 102)
        .unwrap();
    let stopped_timestamps = store
        .export_conversation(&conversation_ref, person)
        .unwrap()
        .conversation
        .activity
        .iter()
        .find_map(|item| match &item.event {
            ConversationActivityEvent::WorkLifecycle {
                state: crate::v2026_09::invocation::ActionInvocationState::Stopped,
                stop_requested_at_unix_ms,
                finished_at_unix_ms,
                ..
            } => Some((*stop_requested_at_unix_ms, *finished_at_unix_ms)),
            _ => None,
        })
        .unwrap();

    store
        .admit(
            &graph,
            &ingress(
                "later-boundary",
                person,
                Some(&conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "Fork after both terminal turns",
            ),
            None,
        )
        .unwrap();
    let selected_message_ref = store
        .conversation(&conversation_ref)
        .unwrap()
        .messages
        .iter()
        .find(|message| message.text == "Fork after both terminal turns")
        .unwrap()
        .message_ref
        .clone();
    let fork = store
        .fork_conversation(
            &graph,
            person,
            &conversation_ref,
            &selected_message_ref,
            "terminal-without-reply",
        )
        .unwrap();

    for expected_status in [ActionResponseStatus::FAILED, ActionResponseStatus::STOPPED] {
        assert!(
            fork.activity.iter().any(|item| matches!(
                &item.event,
                ConversationActivityEvent::WorkUpdate { status, .. }
                    if *status == expected_status
            )),
            "missing {expected_status:?}: {:?}",
            fork.activity
        );
    }
    assert_eq!(
        fork.activity.iter().find_map(|item| match &item.event {
            ConversationActivityEvent::WorkLifecycle {
                state: crate::v2026_09::invocation::ActionInvocationState::Stopped,
                stop_requested_at_unix_ms,
                finished_at_unix_ms,
                ..
            } => Some((*stop_requested_at_unix_ms, *finished_at_unix_ms)),
            _ => None,
        }),
        Some(stopped_timestamps),
    );
}

#[test]
fn later_message_fork_retains_running_work_without_a_reply() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("running-without-reply.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let running = admit_context_turn(&mut store, &graph, None, "running-root", "Start work");
    let conversation_ref = running.conversation.conversation_ref;
    let invocation_ref = running.record.invocation_ref.as_deref().unwrap().to_owned();
    assert!(matches!(
        store.claim_invocation(&invocation_ref, 100).unwrap(),
        ConversationInvocationClaimOutcome::Claimed(_)
    ));
    store
        .record_invocation_progress_with_activity(
            &invocation_ref,
            102,
            "Unpublished progress before the fork point",
        )
        .unwrap();
    store
        .admit(
            &graph,
            &ingress(
                "running-boundary",
                person,
                Some(&conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "Fork after starting work",
            ),
            None,
        )
        .unwrap();
    let selected_message_ref = store
        .conversation(&conversation_ref)
        .unwrap()
        .messages
        .iter()
        .find(|message| message.text == "Fork after starting work")
        .unwrap()
        .message_ref
        .clone();
    let fork = store
        .fork_conversation(
            &graph,
            person,
            &conversation_ref,
            &selected_message_ref,
            "running-without-reply",
        )
        .unwrap();
    assert!(fork.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::WorkUpdate {
            status: ActionResponseStatus::PROGRESS,
            ..
        }
    )));
    assert!(fork.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::WorkUpdate {
            text: Some(text),
            ..
        } if text == "Unpublished progress before the fork point"
    )));
    assert!(fork.activity.iter().any(|item| matches!(
        &item.event,
        ConversationActivityEvent::WorkLifecycle {
            invocation_ref: _,
            state: crate::v2026_09::invocation::ActionInvocationState::Stopped,
            finished_at_unix_ms: Some(_),
            ..
        }
    )));
}
