#[test]
fn canonical_transfer_enforces_inspector_payload_permissions() {
    let (_directory, mut store, conversation, _message, invocation, _root, _parent) =
        populated_canonical_timeline();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let reviewer = "cwp://com.example.fixture/actors/reviewer";
    store.claim_invocation(&invocation, 100).unwrap();
    inspector_tool(
        &mut store,
        &invocation,
        "private-export",
        true,
        "secret inspector output",
    );
    store
        .transact(|state| {
            state
                .conversations
                .find_mut(|c| c.conversation_ref == conversation)
                .unwrap()
                .participant_actor_refs
                .push(reviewer.into());
            Ok(())
        })
        .unwrap();
    for actor in [person, helper] {
        let export = store.export_conversation(&conversation, actor).unwrap();
        assert_eq!(
            export.timeline.as_ref().unwrap().work[0].activities.len(),
            1
        );
        assert!(serde_json::to_string(&export)
            .unwrap()
            .contains("secret inspector output"));
    }
    let export = store.export_conversation(&conversation, reviewer).unwrap();
    assert_eq!(export.timeline.as_ref().unwrap().work.len(), 1);
    assert!(export.timeline.as_ref().unwrap().work[0]
        .activities
        .is_empty());
    let bytes = serde_json::to_string(&export).unwrap();
    assert!(!bytes.contains("secret inspector output"));
    assert!(!bytes.contains("echo private"));
    let imported = store
        .import_conversation(
            &fixture_graph_with_reviewer(),
            reviewer,
            export,
            Some("redacted-inspector"),
        )
        .unwrap();
    let record = store
        .invocation_records()
        .find(|r| r.conversation_ref.as_deref() == Some(&imported.conversation_ref))
        .unwrap();
    assert!(!store
        .state
        .tool_operations
        .values()
        .any(|op| op.invocation_ref == record.invocation_ref));
}

fn transfer_tool_at(
    store: &mut ConversationAdmissionStore,
    invocation: &str,
    id: &str,
    completed: bool,
    time: u64,
    output: &str,
) {
    store.transact(|state| {
        invocation::push_execution_activity_in_state(state, invocation, ConversationActivityEvent::ProviderEvent {
            provider: "codex".into(), method: if completed { "item/completed" } else { "item/started" }.into(),
            data: json!({"item":{"id":id,"type":"commandExecution","command":"echo private","aggregatedOutput":output,"exitCode":0,"status":if completed {"completed"} else {"inProgress"}}}),
        }, time)?;
        Ok(())
    }).unwrap();
}

#[test]
fn canonical_transfer_preserves_tool_order_latest_event_and_settled_state() {
    let (_directory, mut store, conversation, _message, invocation, _root, _parent) =
        populated_canonical_timeline();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    store.claim_invocation(&invocation, 100).unwrap();
    let attempt = store
        .state
        .tool_attempts
        .values()
        .find(|a| a.invocation_ref == invocation)
        .unwrap()
        .reference
        .clone();
    let mut ids = ["order-a", "order-b"];
    ids.sort_by_key(|id| {
        std::cmp::Reverse(crate::cwpdb::derived_ref(
            "tool_operation",
            &[&attempt, "codex", id],
        ))
    });
    transfer_tool_at(&mut store, &invocation, ids[0], false, 500, "old output");
    transfer_tool_at(
        &mut store,
        &invocation,
        ids[1],
        false,
        600,
        "unfinished output",
    );
    // Event sequence moves forward while provider wall clocks move backward.
    transfer_tool_at(
        &mut store,
        &invocation,
        ids[0],
        true,
        200,
        "latest completed output",
    );
    let export = store.export_conversation(&conversation, person).unwrap();
    let activities = &export.timeline.as_ref().unwrap().work[0].activities;
    assert_eq!(activities.len(), 2);
    assert!(activities[0].activity.activity_ref > activities[1].activity.activity_ref);
    assert!(activities[0].first_sequence < activities[1].first_sequence);
    assert!(activities[0].sequence > activities[1].sequence);
    assert_eq!(activities[0].activity.state, "completed");
    assert_eq!(
        activities[0].activity.output.as_deref(),
        Some("latest completed output")
    );
    assert_eq!(activities[1].activity.state, "stopped");
    let count = store.conversations().count();
    for variant in 0..3 {
        let mut invalid = export.clone();
        let items = &mut invalid.timeline.as_mut().unwrap().work[0].activities;
        match variant {
            0 => items[0].first_sequence = 0,
            1 => items[1].activity.activity_ref = items[0].activity.activity_ref.clone(),
            _ => items[0].activity.activity_ref.clear(),
        }
        assert!(store
            .import_conversation(
                &fixture_graph_with_reviewer(),
                person,
                invalid,
                Some("invalid-inspector")
            )
            .is_err());
        assert_eq!(store.conversations().count(), count);
    }
    let imported = store
        .import_conversation(
            &fixture_graph_with_reviewer(),
            person,
            export,
            Some("ordered-inspector"),
        )
        .unwrap();
    for (generation, reference) in [
        ("first", imported.conversation_ref.clone()),
        ("second", {
            let export = store
                .export_conversation(&imported.conversation_ref, person)
                .unwrap();
            store
                .import_conversation(
                    &fixture_graph_with_reviewer(),
                    person,
                    export,
                    Some("reexported-inspector"),
                )
                .unwrap()
                .conversation_ref
        }),
    ] {
        let record = store
            .invocation_records()
            .find(|r| r.conversation_ref.as_deref() == Some(&reference))
            .unwrap();
        let snapshot = inspector_snapshot(&store, person);
        let page = snapshot
            .inspector_page(
                &record.invocation_ref,
                ConversationInspectorCollection::Activities,
                &Default::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(page.entries.len(), 2, "{generation}");
        for (index, expected) in ["latest completed output", "unfinished output"]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                snapshot
                    .inspector_text(
                        &record.invocation_ref,
                        &page.entries[index].0.reference,
                        None,
                        ConversationInspectorTextField::Output
                    )
                    .unwrap()
                    .unwrap()
                    .text,
                expected
            );
        }
        assert!(
            matches!(&page.entries[1].1, ConversationInspectorEntry::Activity { state, active: false, .. } if state == "stopped")
        );
    }
}

#[test]
fn canonical_transfer_preserves_call_order_and_ancestor_inspector_access() {
    use crate::v2026_09::invocation::{InvocationOrigin, StartActionInvocation};
    let (_directory, mut store, conversation, _message, invocation, _root, parent) =
        populated_canonical_timeline();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let claimed = match store.claim_invocation(&invocation, 100).unwrap() {
        ConversationInvocationClaimOutcome::Claimed(claimed) => claimed,
        _ => panic!("claim"),
    };
    for (index, name) in ["z-first", "a-second"].into_iter().enumerate() {
        let child = format!("cwp://com.example.runtime/invocations/{name}");
        let call = store
            .append_action_call(
                &conversation,
                helper,
                helper,
                "@helper--review--work",
                json!({}),
                "Child request".into(),
            )
            .unwrap();
        let mut adapter = crate::adapter::AdapterInvocation::new(
            &child,
            "review--work",
            json!({}),
            json!({"type":"object"}),
        );
        adapter.integration_ref = claimed.adapter_invocation.integration_ref.clone();
        store
            .start_child_invocation(
                StartActionInvocation {
                    invocation_ref: child.clone(),
                    origin: InvocationOrigin::HostCall {
                        caller_invocation_ref: invocation.clone(),
                        call_id: name.into(),
                    },
                    requester_actor_ref: helper.into(),
                    target_actor_ref: helper.into(),
                    conversation_ref: Some(conversation.clone()),
                    actor_integration_ref: claimed.claim.invocation.actor_integration_ref.clone(),
                    action_name: "review--work".into(),
                    input: json!({}),
                    parent_invocation_ref: Some(invocation.clone()),
                    admitted_at_unix_ms: 200 + index as u64,
                },
                &call.message_ref,
                adapter,
            )
            .unwrap();
        store
            .retain_invocation_timeline_publication(
                &child,
                &call.message_ref,
                &format!("{conversation}/publications/{name}"),
                ConversationTimelineDisposition::Child {
                    parent_item_ref: parent.item_ref.clone(),
                },
            )
            .unwrap();
        store.claim_invocation(&child, 300 + index as u64).unwrap();
        inspector_tool(&mut store, &child, name, true, "ancestor-visible output");
    }
    let mut export = store.export_conversation(&conversation, person).unwrap();
    let children = export
        .timeline
        .as_ref()
        .unwrap()
        .work
        .iter()
        .filter(|r| r.parent_invocation_ref.as_deref() == Some(&invocation))
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 2);
    assert!(
        children.iter().all(|r| r.activities.len() == 1),
        "ancestor requester can export inspector details"
    );
    // Foreign counters must not overflow this runtime's next live admission.
    for work in &mut export.timeline.as_mut().unwrap().work {
        work.admission_sequence = if work.invocation_ref.ends_with("z-first") {
            u64::MAX - 1
        } else if work.invocation_ref.ends_with("a-second") {
            u64::MAX
        } else {
            1
        };
    }
    let imported = store
        .import_conversation(
            &fixture_graph_with_reviewer(),
            person,
            export,
            Some("ordered-calls"),
        )
        .unwrap();
    let root = store
        .invocation_records()
        .find(|r| {
            r.conversation_ref.as_deref() == Some(&imported.conversation_ref)
                && r.parent_invocation_ref.is_none()
        })
        .unwrap();
    let snapshot = inspector_snapshot(&store, person);
    let page = snapshot
        .inspector_page(
            &root.invocation_ref,
            ConversationInspectorCollection::Calls,
            &Default::default(),
        )
        .unwrap()
        .unwrap();
    let originals = page
        .entries
        .iter()
        .map(|(_, entry)| {
            let ConversationInspectorEntry::Call {
                callee_invocation_ref: Some(reference),
                ..
            } = entry
            else {
                panic!("call")
            };
            store
                .invocation_record(reference)
                .unwrap()
                .forked_from_invocation_ref
                .as_deref()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(originals[0].ends_with("z-first"));
    assert!(originals[1].ends_with("a-second"));
    drop(snapshot);
    let continued = working_turn(
        &mut store,
        Some(&imported.conversation_ref),
        "ordered-calls-continued",
    );
    assert!(continued.record.invocation_ref.is_some());
}
