fn admit_context_turn(
    store: &mut ConversationAdmissionStore,
    graph: &ResolvedCwrGraph,
    conversation_ref: Option<&str>,
    event: &str,
    text: &str,
) -> AdmittedConversationTurn {
    let mut prepared = store
        .prepare(
            graph,
            &ingress(
                event,
                crate::v2026_09::test_fixture::actor_ref("person"),
                conversation_ref,
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                text,
            ),
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .unwrap();
    if let ConversationPreparationOutcome::New(turn) = &mut prepared {
        // Exercise independent invocation snapshots, including pre-continuity history.
        turn.invocation.as_mut().unwrap().conversation_continuity = false;
    }
    let ConversationAdmissionOutcome::Started(turn) = store.commit(prepared).unwrap() else {
        panic!("expected a new invocation");
    };
    *turn
}

fn retained_context_snapshot(store: &ConversationAdmissionStore, invocation_ref: &str) -> Value {
    store
        .execution_conversation(invocation_ref)
        .unwrap()
        .activity
        .iter()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::TurnStarted {
                invocation: Some(snapshot),
                ..
            } => Some(snapshot.clone()),
            _ => None,
        })
        .unwrap()
}

#[test]
#[ignore = "manual read-scaling characterization"]
fn characterize_context_snapshot_hashing() {
    for message_count in [100, 1_000, 5_000] {
        let messages = (0..message_count)
            .map(|index| {
                json!({
                    "message_ref": format!("cwp://characterization/messages/{index}"),
                    "author_actor_ref": "cwp://com.example.fixture/actors/person",
                    "addressed_actor_refs": ["cwp://com.example.fixture/actors/helper"],
                    "addressing_mode": "explicit",
                    "text": format!("Representative message {index}: {}", "x".repeat(180)),
                })
            })
            .collect::<Vec<_>>();
        let thread = json!({
            "conversation_ref": "cwp://characterization/conversations/history",
            "messages": messages,
        });
        let iterations = 20;
        let started = std::time::Instant::now();
        let mut retained_bytes = 0;
        for _ in 0..iterations {
            let referenced = context::reference(&thread).unwrap();
            retained_bytes = serde_json::to_vec(&referenced).unwrap().len();
        }
        println!(
            "context_snapshot_hashing messages={message_count} source_bytes={} retained_bytes={retained_bytes} average_us={}",
            serde_json::to_vec(&thread).unwrap().len(),
            started.elapsed().as_micros() / iterations,
        );
    }
}

#[test]
fn invocation_context_restores_exact_admission_prefix_after_new_messages_and_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("context.sqlite");
    let graph = fixture_graph_with_reviewer();
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let first = admit_context_turn(
        &mut store,
        &graph,
        None,
        "early",
        "Use the original constraint 🦀",
    );
    let second = admit_context_turn(
        &mut store,
        &graph,
        Some(&first.record.conversation_ref),
        "correction",
        "Correction: preserve every exact byte\n\"quoted\"",
    );
    let source = second.conversation.messages.clone();
    admit_context_turn(
        &mut store,
        &graph,
        Some(&first.record.conversation_ref),
        "later",
        "This arrived after admission",
    );
    let invocation_ref = second.record.invocation_ref.as_deref().unwrap();
    let snapshot = retained_context_snapshot(&store, invocation_ref);
    assert!(snapshot["thread"].get("messages").is_none());
    assert_eq!(snapshot["thread"]["context_snapshot"]["message_count"], 2);
    drop(store);
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let ConversationInvocationClaimOutcome::Claimed(claimed) =
        store.claim_invocation(invocation_ref, 100).unwrap()
    else {
        panic!("claim expected")
    };
    assert_eq!(
        claimed.adapter_invocation.thread["messages"],
        serde_json::to_value(source).unwrap()
    );
    assert!(claimed.adapter_invocation.continuation.is_none());
}

#[test]
fn invocation_context_rejects_changed_canonical_source_without_claiming() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("context.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = admit_context_turn(
        &mut store,
        &fixture_graph_with_reviewer(),
        None,
        "original",
        "original",
    );
    let invocation_ref = first.record.invocation_ref.as_deref().unwrap();
    store
        .state
        .conversations
        .iter_mut()
        .find(|c| c.conversation_ref == first.record.conversation_ref)
        .unwrap()
        .messages[0]
        .text = "changed".into();
    let error = store.claim_invocation(invocation_ref, 100).unwrap_err();
    assert!(
        error.to_string().contains("source changed after admission"),
        "{error}"
    );
    assert_eq!(
        store
            .invocation_record(invocation_ref)
            .unwrap()
            .attempt_count,
        0
    );
}

#[test]
fn invocation_snapshot_storage_grows_with_invocations_not_history_prefixes() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("context.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let graph = fixture_graph_with_reviewer();
    let body = "complete source body 🦀 \"\\\"\n".repeat(1024);
    let mut conversation_ref = None;
    let mut invocation_refs = Vec::new();
    let mut half_bytes = 0;
    for index in 0..32 {
        let admitted = admit_context_turn(
            &mut store,
            &graph,
            conversation_ref.as_deref(),
            &format!("event-{index}"),
            &body,
        );
        conversation_ref = Some(admitted.record.conversation_ref.clone());
        invocation_refs.push(admitted.record.invocation_ref.unwrap());
        if index == 15 {
            half_bytes = invocation_refs
                .iter()
                .map(|reference| {
                    serde_json::to_vec(&retained_context_snapshot(&store, reference))
                        .unwrap()
                        .len()
                })
                .sum::<usize>();
        }
    }
    let all_bytes = invocation_refs
        .iter()
        .map(|reference| {
            let snapshot = serde_json::to_vec(&retained_context_snapshot(&store, reference)).unwrap();
            assert!(
                !snapshot
                    .windows(b"complete source body".len())
                    .any(|window| window == b"complete source body"),
                "snapshot unexpectedly contains a source body"
            );
            snapshot.len()
        })
        .sum::<usize>();
    assert!(all_bytes < half_bytes * 21 / 10, "doubling history and invocations must not quadruple snapshots: {half_bytes} -> {all_bytes}");
    let ConversationInvocationClaimOutcome::Claimed(claimed) = store
        .claim_invocation(invocation_refs.last().unwrap(), 100)
        .unwrap()
    else {
        panic!("claim expected")
    };
    let messages = claimed.adapter_invocation.thread["messages"]
        .as_array()
        .unwrap();
    assert_eq!(messages.len(), 32);
    assert!(messages.iter().all(|message| message["text"] == body));
}

#[test]
fn legacy_inline_context_migrates_without_changing_its_boundary() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("context.sqlite");
    let graph = fixture_graph_with_reviewer();
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let first = admit_context_turn(&mut store, &graph, None, "original", "old history");
    let invocation_ref = first.record.invocation_ref.as_deref().unwrap();
    let mut state = store.state.clone();
    let execution = state
        .conversations
        .iter_mut()
        .find(|c| Some(&c.conversation_ref) == first.record.execution_conversation_ref.as_ref())
        .unwrap();
    let ConversationActivityEvent::TurnStarted {
        invocation: Some(snapshot),
        ..
    } = &mut execution.activity[0].event
    else {
        panic!("start expected")
    };
    snapshot["thread"] = conversation_thread(&first.conversation);
    store.replace_state(state).unwrap();
    admit_context_turn(
        &mut store,
        &graph,
        Some(&first.record.conversation_ref),
        "later",
        "not in the old snapshot",
    );
    drop(store);
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    assert!(retained_context_snapshot(&store, invocation_ref)["thread"]
        .get("messages")
        .is_none());
    let ConversationInvocationClaimOutcome::Claimed(claimed) =
        store.claim_invocation(invocation_ref, 100).unwrap()
    else {
        panic!("claim expected")
    };
    assert_eq!(
        claimed.adapter_invocation.thread["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        claimed.adapter_invocation.thread["messages"][0]["text"],
        "old history"
    );
}

#[test]
fn exceptional_legacy_source_remains_exact_when_canonical_history_differs() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("context.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let first = admit_context_turn(
        &mut store,
        &fixture_graph_with_reviewer(),
        None,
        "original",
        "original source",
    );
    let invocation_ref = first.record.invocation_ref.as_deref().unwrap();
    let mut state = store.state.clone();
    let execution = state
        .conversations
        .iter_mut()
        .find(|c| Some(&c.conversation_ref) == first.record.execution_conversation_ref.as_ref())
        .unwrap();
    let ConversationActivityEvent::TurnStarted {
        invocation: Some(snapshot),
        ..
    } = &mut execution.activity[0].event
    else {
        panic!("start expected")
    };
    snapshot["thread"] = conversation_thread(&first.conversation);
    state
        .conversations
        .iter_mut()
        .find(|c| c.conversation_ref == first.record.conversation_ref)
        .unwrap()
        .messages[0]
        .text = "later canonical content".into();
    store.replace_state(state).unwrap();
    drop(store);
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let ConversationInvocationClaimOutcome::Claimed(claimed) =
        store.claim_invocation(invocation_ref, 100).unwrap()
    else {
        panic!("claim expected")
    };
    assert_eq!(
        claimed.adapter_invocation.thread["messages"][0]["text"],
        "original source"
    );
    store
        .retain_execution_invocation(invocation_ref, &claimed.adapter_invocation)
        .unwrap();
    drop(store);
    let store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    assert_eq!(
        retained_context_snapshot(&store, invocation_ref)["thread"]["messages"][0]["text"],
        "original source"
    );
}
