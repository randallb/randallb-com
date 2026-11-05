use super::*;
use cwr::v2026_09::{
    host::{measure_turn_status_projection, ConversationTurnState},
    invocation::ActionInvocationState,
};

enum StopRoute {
    NamedAction,
    TextMessage,
}

#[test]
fn scheduled_stop_action_clears_another_conversations_pending_status() {
    exercise_cross_conversation_stop(StopRoute::NamedAction);
}

#[test]
fn scheduled_stop_text_clears_another_conversations_pending_status() {
    exercise_cross_conversation_stop(StopRoute::TextMessage);
}

#[test]
fn scheduled_readonly_control_skips_unrelated_worker_histories() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let person = crate::test_refs::actor_ref("person");
    let b = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "readonly-unrelated",
            &person,
            None,
            "@helper unrelated pending work",
        )
        .unwrap();
    let control = runtime
        .admit_action_as(
            "cwp://example.test/integrations/desktop",
            "readonly-control",
            &person,
            None,
            "@cwr--list--actions",
            serde_json::json!({}),
        )
        .unwrap();
    broadcast_turn_statuses(&state, &runtime).unwrap();
    let b_status =
        lock_state(&state).unwrap().published_turn_statuses[b.conversation_ref()].clone();
    let task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(
        task.record().invocation_ref,
        control.invocation_ref().unwrap()
    );
    let (_, work) = measure_turn_status_projection(|| {
        scheduler::execute(Arc::clone(&state), runtime.clone(), task);
    });
    assert!(!work.is_empty());
    assert!(
        work.iter()
            .all(|(reference, _)| reference == control.conversation_ref()),
        "read-only control reconstructed unrelated history: {work:?}"
    );
    let turn = runtime.turn(&control.record).unwrap().invocation.unwrap();
    assert_eq!(turn.state, ActionInvocationState::Completed);
    let published = &lock_state(&state).unwrap().published_turn_statuses;
    assert_eq!(published[b.conversation_ref()], b_status);
    assert_eq!(
        *published,
        runtime.conversation_host().turn_statuses().unwrap()
    );
}

#[test]
fn experimental_feature_reads_keep_the_mutating_control_refresh() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let control = runtime
        .admit_action(
            None,
            "@cwr--read--experimental_features",
            serde_json::json!({}),
        )
        .unwrap();
    // Admission and classification do not execute the action or touch the
    // person's experimental-feature settings. The read also applies its graph.
    assert!(
        runtime
            .conversation_host()
            .turn_status_publication_for_execution(
                &BTreeSet::from([control.invocation_ref().unwrap().to_owned()]),
                &BTreeSet::from([control.conversation_ref().to_owned()]),
                &BTreeMap::new(),
            )
            .unwrap()
            .replace_all
    );
}

fn exercise_cross_conversation_stop(route: StopRoute) {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let person = crate::test_refs::actor_ref("person");
    let admit = |event, text| {
        runtime
            .submit_text_as(
                "cwp://example.test/integrations/desktop",
                event,
                &person,
                None,
                text,
            )
            .unwrap()
    };
    let a = admit("stop-control-owner", "Control conversation");
    let b = admit("stop-control-target", "@helper pending target");
    let c = admit("stop-control-unrelated", "@helper unrelated work");
    let control = match route {
        StopRoute::NamedAction => runtime
            .admit_action_as(
                "cwp://example.test/integrations/desktop",
                "scheduled-stop",
                &person,
                Some(a.conversation_ref()),
                "@cwr--stop--invocation",
                serde_json::json!({"invocation_ref": b.invocation_ref().unwrap()}),
            )
            .unwrap(),
        StopRoute::TextMessage => runtime
            .admit_conversation_text(
                Some(a.conversation_ref()),
                format!("@cwr stop {}", b.invocation_ref().unwrap()),
            )
            .unwrap(),
    };
    assert_ne!(control.conversation_ref(), b.conversation_ref());
    broadcast_turn_statuses(&state, &runtime).unwrap();
    let c_status = {
        let published = &lock_state(&state).unwrap().published_turn_statuses;
        assert_eq!(
            published[b.conversation_ref()][0].state,
            ConversationTurnState::Pending
        );
        published[c.conversation_ref()].clone()
    };
    let (client, server) = wire::wire_pair();
    let writer = client_writer(server).unwrap();
    subscribe(&state, &writer).unwrap();
    let reader = read_until_sentinel(client);
    let task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(
        task.record().invocation_ref,
        control.invocation_ref().unwrap()
    );
    scheduler::execute(Arc::clone(&state), runtime.clone(), task);
    broadcast_execution_failure(
        &state,
        a.conversation_ref(),
        "scheduled-stop-sentinel",
        "fixture",
        "end fixture",
    )
    .unwrap();
    let events = reader.join().unwrap();
    disconnect_writer(&writer);

    assert_eq!(
        runtime.turn(&b.record).unwrap().invocation.unwrap().state,
        ActionInvocationState::Stopped
    );
    // No B worker existed to produce its own event or a later settled-stop
    // notification. The scheduled control's publication must clear B now.
    assert!(runtime.take_settled_stop_invocations().unwrap().is_empty());
    assert!(events.iter().any(|event| matches!(event,
        RuntimeServiceEvent::ConversationExecutionFinished { response, .. }
            if response.invocation_ref == control.invocation_ref().unwrap() && response.status.as_u16() == 200)));
    assert!(
        events.iter().any(|event| matches!(event,
        RuntimeServiceEvent::ConversationTurnStatusSnapshot { conversation_ref, turns }
            if conversation_ref == b.conversation_ref() && turns.is_empty())),
        "scheduled stop did not clear the target status: {events:?}"
    );
    assert!(!events.iter().any(|event| matches!(event,
        RuntimeServiceEvent::ConversationTurnStatusSnapshot { conversation_ref, .. }
            if conversation_ref == c.conversation_ref())));
    let published = &lock_state(&state).unwrap().published_turn_statuses;
    assert!(!published.contains_key(b.conversation_ref()));
    assert_eq!(published[c.conversation_ref()], c_status);
    assert_eq!(
        *published,
        runtime.conversation_host().turn_statuses().unwrap()
    );
}

fn read_until_sentinel(client: UnixStream) -> thread::JoinHandle<Vec<RuntimeServiceEvent>> {
    thread::spawn(move || {
        let mut events = Vec::new();
        for line in BufReader::new(client).lines() {
            let event: RuntimeServiceEvent = serde_json::from_str(&line.unwrap()).unwrap();
            let finished = matches!(&event,
                RuntimeServiceEvent::ConversationExecutionFailed { invocation_ref, .. }
                    if invocation_ref == "scheduled-stop-sentinel");
            events.push(event);
            if finished {
                return events;
            }
        }
        panic!("scheduled stop fixture ended without its sentinel")
    })
}
