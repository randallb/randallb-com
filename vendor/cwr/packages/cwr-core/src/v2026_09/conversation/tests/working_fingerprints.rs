fn set_serialized_working_fingerprint(
    store: &mut ConversationAdmissionStore,
    invocation_ref: &str,
    field: &str,
    value: Option<&str>,
) {
    let execution_ref = store
        .execution_conversation_ref(invocation_ref)
        .unwrap()
        .to_owned();
    let mut state = store.state.clone();
    let execution = state
        .conversations
        .iter_mut()
        .find(|conversation| conversation.conversation_ref == execution_ref)
        .unwrap();
    let snapshot = execution
        .activity
        .iter_mut()
        .find_map(|activity| match &mut activity.event {
            ConversationActivityEvent::TurnStarted {
                invocation: Some(snapshot),
                ..
            } => Some(snapshot),
            _ => None,
        })
        .unwrap()
        .as_object_mut()
        .unwrap();
    if let Some(value) = value {
        snapshot.insert(field.into(), json!(value));
    } else {
        snapshot.remove(field);
    }
    store.replace_state(state).unwrap();
}

#[test]
fn legacy_private_history_reopens_and_continues_with_the_same_route() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy-continuity.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let first = working_turn(&mut store, None, "first request");
    let claimed = claim_working(&mut store, &first);
    assert_ne!(
        claimed.adapter_invocation.provider_fingerprint,
        claimed.adapter_invocation.route_fingerprint
    );
    settle_working(&mut store, &claimed);
    set_serialized_working_fingerprint(
        &mut store,
        &claimed.adapter_invocation.invocation_ref,
        "provider_fingerprint",
        None,
    );
    drop(store);

    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let previous = retained_context_snapshot(&store, &claimed.adapter_invocation.invocation_ref);
    assert!(previous.get("provider_fingerprint").is_none());
    assert_eq!(
        previous["route_fingerprint"],
        json!(claimed.adapter_invocation.route_fingerprint)
    );
    let next = working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "followup request",
    );
    let followup = claim_working(&mut store, &next);
    assert!(followup.adapter_invocation.provider_fingerprint.is_some());
    assert_eq!(
        followup.adapter_invocation.route_fingerprint,
        claimed.adapter_invocation.route_fingerprint
    );
    assert_eq!(
        followup.adapter_invocation.thread["working_conversation"]["ref"],
        claimed.adapter_invocation.thread["working_conversation"]["ref"]
    );
    assert_eq!(
        followup.adapter_invocation.thread["working_conversation"]["previous_invocation_ref"],
        claimed.adapter_invocation.invocation_ref
    );
    assert_eq!(
        followup.adapter_invocation.thread["working_conversation"]["state"]["thread_id"],
        "private-provider-thread"
    );
    assert_eq!(followup.adapter_invocation.thread["context_start"], 1);
    assert_eq!(
        followup.adapter_invocation.thread["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Done", "followup request"]
    );
}

#[test]
fn legacy_private_history_rejects_changed_or_missing_route_fingerprints_after_reopen() {
    for (previous_route, current_route) in [
        (Some("previous-route"), Some("changed-route")),
        (None, Some("current-route")),
        (Some("previous-route"), None),
        (None, None),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy-continuity.sqlite");
        let mut store =
            ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
        let first = working_turn(&mut store, None, "first request");
        let claimed = claim_working(&mut store, &first);
        settle_working(&mut store, &claimed);
        set_serialized_working_fingerprint(
            &mut store,
            &claimed.adapter_invocation.invocation_ref,
            "provider_fingerprint",
            None,
        );
        set_serialized_working_fingerprint(
            &mut store,
            &claimed.adapter_invocation.invocation_ref,
            "route_fingerprint",
            previous_route,
        );
        let next = working_turn(
            &mut store,
            Some(&first.record.conversation_ref),
            "followup request",
        );
        let next_ref = next.record.invocation_ref.as_deref().unwrap();
        set_serialized_working_fingerprint(
            &mut store,
            next_ref,
            "route_fingerprint",
            current_route,
        );
        drop(store);

        let mut store =
            ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
        let error = store
            .claim_invocation(next_ref, 102)
            .unwrap_err()
            .to_string();
        assert!(error.contains("execution configuration changed"), "{error}");
        assert_eq!(store.invocation_record(next_ref).unwrap().attempt_count, 0);
    }
}

#[test]
fn modern_private_history_rejects_changed_provider_hash_even_when_route_hash_matches() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("modern-continuity.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let first = working_turn(&mut store, None, "first request");
    let claimed = claim_working(&mut store, &first);
    settle_working(&mut store, &claimed);
    let next = working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "followup request",
    );
    let next_ref = next.record.invocation_ref.as_deref().unwrap();
    set_serialized_working_fingerprint(
        &mut store,
        next_ref,
        "provider_fingerprint",
        Some("changed-provider-configuration"),
    );
    drop(store);

    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    assert_eq!(
        retained_context_snapshot(&store, next_ref)["route_fingerprint"],
        json!(claimed.adapter_invocation.route_fingerprint)
    );
    let error = store
        .claim_invocation(next_ref, 102)
        .unwrap_err()
        .to_string();
    assert!(error.contains("execution configuration changed"), "{error}");
    assert_eq!(store.invocation_record(next_ref).unwrap().attempt_count, 0);
}


#[test]
fn activated_configuration_starts_fresh_private_history_in_the_same_public_conversation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("refresh.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let first = working_turn(&mut store, None, "first request");
    let claimed = claim_working(&mut store, &first);
    settle_working(&mut store, &claimed);
    let next = working_turn_configuration(&mut store, Some(&first.record.conversation_ref), "after refresh", Some("changed-model"));
    let next_ref = next.record.invocation_ref.as_deref().unwrap().to_owned();
    drop(store);
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let followup = claim_working(&mut store, &next);
    let working = &followup.adapter_invocation.thread["working_conversation"];
    assert!(working["history_gap"].as_str().unwrap().contains("execution configuration changed"));
    assert!(working.get("state").is_none());
    assert_ne!(working["ref"], claimed.adapter_invocation.thread["working_conversation"]["ref"]);
    assert_eq!(next.record.conversation_ref, first.record.conversation_ref);
    assert!(serde_json::to_string(&followup.adapter_invocation.thread).unwrap().contains("first request"));
    assert_eq!(store.invocation_record(&next_ref).unwrap().attempt_count, 1);
}
