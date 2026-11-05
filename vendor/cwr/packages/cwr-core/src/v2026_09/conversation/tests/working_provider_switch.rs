#[test]
fn provider_switch_keeps_shared_history_separate_private_state_and_exact_retry() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("switch.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let first = working_turn(&mut store, None, "Codex request");
    let claimed = claim_working(&mut store, &first);
    settle_working(&mut store, &claimed);
    let mut graph = fixture_graph_with_reviewer();
    let primary = crate::v2026_09::test_fixture::binding_ref("helper");
    let mut binding = graph.actor_integrations.iter().find(|b| b.actor_integration_ref == primary).unwrap().clone();
    binding.actor_integration_ref = format!("{}/bindings/claude", binding.actor_ref);
    binding.adapter_ref = crate::adapter::CLAUDE_ADAPTER_REF.into();
    let secondary = binding.actor_integration_ref.clone();
    graph.actor_integrations.push(binding);
    let incoming = ingress("Claude request", crate::v2026_09::test_fixture::actor_ref("person"), Some(&first.record.conversation_ref), ConversationIngressAddressing::Addressed { actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()] }, "Continue shared work");
    let ConversationAdmissionOutcome::Started(second) = store.admit(&graph, &incoming, Some(&secondary)).unwrap() else { panic!("start") };
    let second_claim = claim_working(&mut store, &second);
    let thread = &second_claim.adapter_invocation.thread;
    assert_ne!(thread["working_conversation"]["ref"], claimed.adapter_invocation.thread["working_conversation"]["ref"]);
    assert!(thread["working_conversation"].get("state").is_none());
    assert!(thread["messages"].as_array().unwrap().iter().any(|message| message["text"] == "Codex request"));
    assert!(thread["messages"].as_array().unwrap().iter().any(|message| message["text"] == "Done"));
    store.recover_interrupted_invocations().unwrap();
    drop(store);
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let replay = store.admit(&graph, &incoming, Some(primary)).unwrap();
    let ConversationAdmissionOutcome::Existing(record) = replay else { panic!("replay") };
    assert_eq!(record.actor_integration_ref.as_deref(), Some(secondary.as_str()));
    assert_eq!(record.invocation_ref, second.record.invocation_ref);
    let retried = claim_working(&mut store, &second);
    assert_eq!(retried.claim.invocation.actor_integration_ref.as_deref(), Some(secondary.as_str()));
    assert_eq!(retried.claim.invocation.attempt_count, 2);
    assert_eq!(retried.adapter_invocation.thread["messages"], thread["messages"]);
    // Recovering the interrupted attempt preserves the retained provider and
    // source boundary rather than replaying partially executed work elsewhere.
    let retained = retained_context_snapshot(&store, second.record.invocation_ref.as_deref().unwrap());
    assert_eq!(retained["retained_route"]["adapter_ref"], crate::adapter::CLAUDE_ADAPTER_REF);
    assert_eq!(context::restore(&store.state, &retained["thread"]).unwrap()["messages"], thread["messages"]);
}
