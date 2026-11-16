fn legacy_runtime_state(temporary: &tempfile::TempDir) -> SharedState {
    runtime_state_with_options(
        temporary,
        0,
        cwr::v2026_09::local::LocalRuntimeOptions {
            conversation_timeline_rollout:
                cwr::v2026_09::conversation::ConversationTimelineRollout::Disabled,
            ..Default::default()
        },
    )
}

use super::*;

#[test]
fn gateway_projection_removes_visualization_paths_and_metadata() {
    let message: cwr::v2026_09::conversation::ConversationMessage =
        serde_json::from_value(serde_json::json!({
            "message_ref":"cwp://example.test/messages/visualization",
            "author_actor_ref":"cwp://example.test/actors/helper",
            "addressed_actor_refs":[],
            "text":"Summary\n\n\u{e200}visualize\u{e202}{\"path\":\"/Users/example/private/chart.html\",\"title\":\"Saved chart\"}\u{e201}",
            "visualizations":[{
                "marker_index":0,
                "title":"Saved chart",
                "artifact_ref":"cwp://example.test/visualizations/private"
            }]
        }))
        .unwrap();

    let delivered = message.clone_for_external_delivery();
    assert_eq!(delivered.text, "Summary\n\nSaved chart");
    assert!(delivered.visualizations.is_empty());
    let serialized = serde_json::to_string(&delivered).unwrap();
    assert!(!serialized.contains("/Users/example/private"));
    assert!(!serialized.contains("visualizations/private"));
}

#[test]
fn status_delivered_in_a_snapshot_is_cleared_by_later_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let root = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "snapshot-status",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@helper queued status",
        )
        .unwrap();
    snapshot_events(&state, "initial-status");
    let (client, server) = wire::wire_pair();
    let writer = client_writer(server).unwrap();
    subscribe(&state, &writer).unwrap();
    runtime
        .stop_invocation_as(
            root.invocation_ref().unwrap(),
            crate::test_refs::actor_ref("person").as_str(),
            None,
        )
        .unwrap();
    broadcast_turn_statuses(&state, &runtime).unwrap();
    let received = wire::read_events(client, 1).join().unwrap();
    assert!(
        matches!(&received[0], RuntimeServiceEvent::ConversationTurnStatusSnapshot { turns, .. } if turns.is_empty())
    );
    disconnect_writer(&writer);
}

#[test]
fn turn_status_publication_sends_changes_and_clears_finished_work() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let root = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "queued-status",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@helper queued status",
        )
        .unwrap();
    let (client, server) = wire::wire_pair();
    let writer = client_writer(server).unwrap();
    subscribe(&state, &writer).unwrap();
    broadcast_turn_statuses(&state, &runtime).unwrap();
    broadcast_turn_statuses(&state, &runtime).unwrap();
    let initial = wire::read_events(client.try_clone().unwrap(), 1)
        .join()
        .unwrap();
    assert!(
        matches!(&initial[0], RuntimeServiceEvent::ConversationTurnStatusSnapshot { turns, .. } if turns.len() == 1)
    );
    runtime
        .stop_invocation_as(
            root.invocation_ref().unwrap(),
            crate::test_refs::actor_ref("person").as_str(),
            None,
        )
        .unwrap();
    broadcast_turn_statuses(&state, &runtime).unwrap();
    let received = wire::read_events(client, 1).join().unwrap();
    assert!(
        matches!(&received[0], RuntimeServiceEvent::ConversationTurnStatusSnapshot { turns, .. } if turns.is_empty())
    );
    disconnect_writer(&writer);
}

#[test]
fn reply_and_progress_delivery_carry_the_root_invocation_and_source_messages_do_not() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = legacy_runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let root = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "progress-root",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@helper check this",
        )
        .unwrap();
    let invocation_ref = root.record.invocation_ref.as_deref().unwrap();
    // Complete through the store while the runtime is closed, then reopen it.
    // This also checks that delivery correlation survives a process restart.
    drop(runtime);
    drop(state);
    {
        let mut store = cwr::v2026_09::conversation::ConversationAdmissionStore::open(
            temporary.path().join("conversation-admission.json"),
            "cwp://example.test/runtime",
        )
        .unwrap();
        store.claim_invocation(invocation_ref, 10).unwrap();
        store
            .record_invocation_progress_with_publication(invocation_ref, 102, "Still working", true)
            .unwrap();
        store
            .complete(
                invocation_ref,
                serde_json::json!({"text": "Here is the answer"}),
            )
            .unwrap();
    }
    let state = legacy_runtime_state(&temporary);
    seed_attachment(
        &state,
        "cwp://example.test/runtime/attachments/progress",
        &root,
        Some(SLACK_BINDING_KEY),
        "a different source message",
    );
    let (client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    lock_state(&state).unwrap().communication_adapters.insert(
        SLACK_INTEGRATION_REF.into(),
        RegisteredCommunicationAdapter {
            _session_ref: "cwp://example.test/sessions/slack".into(),
            destinations: Vec::new(),
            writer: client_writer(server).unwrap(),
        },
    );

    attachments::request_deliveries(&state, root.conversation_ref()).unwrap();
    let requests: Vec<_> = BufReader::new(client)
        .lines()
        .take(3)
        .map(|line| {
            let RuntimeServiceEvent::DeliverMessage { request } =
                serde_json::from_str(&line.unwrap()).unwrap()
            else {
                panic!("expected a message delivery")
            };
            request
        })
        .collect();
    assert_eq!(
        requests[0].message.message_ref,
        root.record.source_message_ref
    );
    assert_eq!(requests[0].invocation_ref, None);
    assert_eq!(requests[1].invocation_ref.as_deref(), Some(invocation_ref));
    assert_eq!(requests[1].message.text, "Still working");
    assert!(requests[1].message.progress.is_some());
    assert_eq!(requests[2].invocation_ref.as_deref(), Some(invocation_ref));
    assert_eq!(requests[2].message.text, "Here is the answer");
    assert_eq!(
        requests[2].message.author_actor_ref,
        crate::test_refs::actor_ref("helper").as_str()
    );
}

#[test]
fn nested_progress_event_publishes_the_shared_root_conversation() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = legacy_runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let root = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "nested-progress-root",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@helper check this",
        )
        .unwrap();
    let root_invocation_ref = root.invocation_ref().unwrap().to_owned();
    let root_execution_ref = root.record.execution_conversation_ref.clone().unwrap();
    let root_actor_ref = root.record.target_actor_ref.clone().unwrap();
    let root_integration_ref = root.record.actor_integration_ref.clone();
    let root_conversation_ref = root.conversation_ref().to_owned();
    drop(runtime);
    drop(state);

    let event = {
        let mut store = cwr::v2026_09::conversation::ConversationAdmissionStore::open(
            temporary.path().join("conversation-admission.json"),
            "cwp://example.test/runtime",
        )
        .unwrap();
        let cwr::v2026_09::conversation::ConversationInvocationClaimOutcome::Claimed(root_claim) =
            store.claim_invocation(&root_invocation_ref, 10).unwrap()
        else {
            panic!("claim root")
        };
        let child_ref = "cwp://example.test/runtime/invocations/nested-progress";
        let call = store
            .append_action_call(
                &root_execution_ref,
                &root_actor_ref,
                &root_actor_ref,
                "@helper--respond_to--message",
                serde_json::json!({"source_message_ref": "nested"}),
                "nested progress".into(),
            )
            .unwrap();
        let mut child_adapter = root_claim.adapter_invocation.clone();
        child_adapter.invocation_ref = child_ref.into();
        child_adapter.conversation_continuity = false;
        child_adapter.thread = serde_json::json!({});
        let child_input = child_adapter.input.clone();
        let child_action_name = child_adapter.action_name.clone();
        store
            .start_child_invocation(
                cwr::v2026_09::invocation::StartActionInvocation {
                    invocation_ref: child_ref.into(),
                    origin: cwr::v2026_09::invocation::InvocationOrigin::HostCall {
                        caller_invocation_ref: root_invocation_ref.clone(),
                        call_id: "nested-progress".into(),
                    },
                    requester_actor_ref: root_actor_ref.clone(),
                    target_actor_ref: root_actor_ref.clone(),
                    conversation_ref: Some(root_execution_ref.clone()),
                    actor_integration_ref: root_integration_ref,
                    action_name: child_action_name,
                    input: child_input,
                    parent_invocation_ref: Some(root_invocation_ref),
                    admitted_at_unix_ms: 11,
                },
                &call.message_ref,
                child_adapter,
            )
            .unwrap();
        store.claim_invocation(child_ref, 12).unwrap();
        store
            .record_invocation_progress_with_publication(child_ref, 102, "Nested progress", true)
            .unwrap();
        let progress = store
            .conversation(&root_conversation_ref)
            .unwrap()
            .messages
            .iter()
            .find_map(|message| message.progress.as_ref())
            .unwrap();
        let activity = store
            .execution_conversation(child_ref)
            .unwrap()
            .activity
            .iter()
            .find(|activity| activity.activity_ref == progress.source_activity_ref)
            .unwrap()
            .clone();
        LocalRuntimeEvent::Activity {
            invocation_ref: child_ref.into(),
            target_actor_ref: root_actor_ref,
            activity: Box::new(activity),
        }
    };

    let state = legacy_runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    assert_eq!(
        event_conversation_ref(&runtime, &event, &root_execution_ref),
        root_conversation_ref
    );
}

#[test]
fn reconnect_snapshot_explicitly_clears_finished_turns() {
    let temporary = tempfile::tempdir().expect("create runtime state");
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    // Exceed typical socket buffers so this also exercises snapshot backpressure.
    let text = format!("@helper {}", "x".repeat(512 * 1024));
    let root = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "stopped-before-reconnect",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            &text,
        )
        .unwrap();
    runtime
        .stop_invocation_as(
            root.record.invocation_ref.as_deref().unwrap(),
            crate::test_refs::actor_ref("person").as_str(),
            None,
        )
        .unwrap();
    let mut snapshots = Vec::new();
    let mut retained_stop = false;
    for event in snapshot_events(&state, "slack-reconnect") {
        if let RuntimeServiceEvent::ConversationSnapshot { conversations } = &event {
            retained_stop = conversations.iter()
                .filter(|conversation| conversation.conversation_ref == root.conversation_ref())
                .flat_map(|conversation| &conversation.activity)
                .any(|activity| matches!(&activity.event,
                    ConversationActivityEvent::WorkUpdate { status, source_message_ref, .. }
                    if status.as_u16() == 499 && source_message_ref == &root.record.source_message_ref));
        }
        if let RuntimeServiceEvent::ConversationTurnStatusSnapshot {
            conversation_ref,
            turns,
        } = event
        {
            snapshots.push((conversation_ref, turns));
        }
    }
    let (_, turns) = snapshots
        .iter()
        .find(|(reference, _)| reference == root.conversation_ref())
        .expect("the stopped conversation has an explicit turn snapshot");
    assert!(turns.is_empty());
    assert!(
        retained_stop,
        "reconnect keeps the stopped outcome beside its request"
    );
}
