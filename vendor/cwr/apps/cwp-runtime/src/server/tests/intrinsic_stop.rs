use super::*;

#[test]
fn intrinsic_stops_publish_pending_state_without_a_worker() {
    {
        let temporary = tempfile::tempdir().unwrap();
        let state = runtime_state(&temporary);
        let runtime = lock_state(&state).unwrap().runtime.clone();
        let root = runtime
            .submit_text_as(
                "cwp://example.test/integrations/desktop",
                "pending-stop",
                crate::test_refs::actor_ref("person").as_str(),
                None,
                "@helper hold this request",
            )
            .unwrap();
        let invocation_ref = root.invocation_ref().unwrap();
        let before = runtime.conversations().unwrap();
        let messages = runtime
            .conversation(root.conversation_ref())
            .unwrap()
            .unwrap()
            .messages;
        let (client, server) = wire::wire_pair();
        let received = wire::read_events(client, 3);
        let writer = client_writer(server).unwrap();
        lock_state(&state)
            .unwrap()
            .subscribers
            .push(Arc::clone(&writer));

        // Select the retained invocation without a current conversation or worker.
        let command = stop_command(invocation_ref);
        execute_command(&state, &writer, command).unwrap();

        let mut events = received.join().unwrap().into_iter();
        let RuntimeServiceEvent::CommandSucceeded {
            admission: None,
            intrinsic_response: Some(response),
            ..
        } = events.next().unwrap()
        else {
            panic!("stop must return an intrinsic response, not admit another message")
        };
        assert!(
            matches!(response.as_ref(), IntrinsicCwrResponse::InvocationStopped {
            invocation_ref: reference, ..
        } if reference == invocation_ref)
        );
        let RuntimeServiceEvent::ConversationUpsert { conversation } = events.next().unwrap()
        else {
            panic!("stop must publish the updated conversation")
        };
        assert_eq!(conversation.conversation_ref, root.conversation_ref());
        assert_eq!(conversation.messages, messages);
        assert!(
            matches!(events.next().unwrap(), RuntimeServiceEvent::ConversationTurnStatusSnapshot {
            conversation_ref, turns,
        } if conversation_ref == root.conversation_ref() && turns.is_empty())
        );

        assert_eq!(runtime.conversations().unwrap().len(), before.len());
        assert!(runtime.conversation_turn_statuses().unwrap().is_empty());
        assert_eq!(
            runtime
                .turn(&root.record)
                .unwrap()
                .invocation
                .unwrap()
                .state,
            cwr::v2026_09::invocation::ActionInvocationState::Stopped
        );
    }
}

#[test]
fn intrinsic_stop_still_publishes_when_the_requester_disconnects() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let root = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "disconnected-stop",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@helper hold this request",
        )
        .unwrap();
    let (subscriber, subscription) = wire::wire_pair();
    let received = wire::read_events(subscriber, 2);
    lock_state(&state)
        .unwrap()
        .subscribers
        .push(client_writer(subscription).unwrap());
    let (requester, request) = wire::wire_pair();
    let requester_writer = client_writer(request).unwrap();
    drop(requester);

    execute_command(
        &state,
        &requester_writer,
        stop_command(root.invocation_ref().unwrap()),
    )
    .expect("stop publication does not depend on the requester draining its response");
    let mut events = received.join().unwrap().into_iter();
    assert!(matches!(
        events.next().unwrap(),
        RuntimeServiceEvent::ConversationUpsert { .. }
    ));
    assert!(
        matches!(events.next().unwrap(), RuntimeServiceEvent::ConversationTurnStatusSnapshot {
        turns, ..
    } if turns.is_empty())
    );
}

fn stop_command(invocation_ref: &str) -> RuntimeServiceCommand {
    RuntimeServiceCommand::ExecuteIntrinsic {
        version: RUNTIME_SERVICE_VERSION,
        request_id: "intrinsic-stop".into(),
        operation_id: None,
        command: IntrinsicCwrCommand::StopInvocation {
            invocation_ref: Some(invocation_ref.into()),
        },
        requesting_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
        conversation_ref: None,
    }
}
