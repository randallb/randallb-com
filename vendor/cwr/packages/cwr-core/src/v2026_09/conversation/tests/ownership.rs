#[test]
fn ownership_survives_continuation_retry_and_restart() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("ownership.sqlite");
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let mut first = ingress(
        "owned-create",
        person,
        None,
        ConversationIngressAddressing::Unaddressed,
        "Brief",
    );
    first.owner_context_ref = Some("writing-pack-instance".into());
    let ConversationAdmissionOutcome::Started(started) = store.admit(&graph, &first, None).unwrap()
    else {
        panic!("new conversation")
    };
    let reference = started.conversation.conversation_ref;
    assert_eq!(
        started.conversation.owner_context_ref,
        first.owner_context_ref
    );
    assert!(matches!(
        store.admit(&graph, &first, None).unwrap(),
        ConversationAdmissionOutcome::Existing(_)
    ));
    let mut changed_retry = first.clone();
    changed_retry.owner_context_ref = Some("other-pack".into());
    assert!(store.admit(&graph, &changed_retry, None).is_err());
    let next = ingress(
        "owned-continue",
        person,
        Some(&reference),
        ConversationIngressAddressing::Unaddressed,
        "Continue",
    );
    store.admit(&graph, &next, None).unwrap();
    let mut asserted_retry = next.clone();
    asserted_retry.owner_context_ref = first.owner_context_ref.clone();
    assert!(matches!(
        store.admit(&graph, &asserted_retry, None).unwrap(),
        ConversationAdmissionOutcome::Existing(_)
    ));
    asserted_retry.owner_context_ref = Some("different-owner".into());
    assert!(store.admit(&graph, &asserted_retry, None).is_err());
    let before = store.conversation(&reference).unwrap().clone();
    for context in ["other-pack", " "] {
        let mut moved = next.clone();
        moved.provider_event_ref = format!("move-{context}");
        moved.owner_context_ref = Some(context.into());
        assert!(store.admit(&graph, &moved, None).is_err());
        assert_eq!(store.conversation(&reference).unwrap(), &before);
    }
    drop(store);
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    assert_eq!(
        store.conversation(&reference).unwrap().owner_context_ref,
        first.owner_context_ref
    );
    assert!(matches!(
        store.admit(&graph, &first, None).unwrap(),
        ConversationAdmissionOutcome::Existing(_)
    ));
    assert!(matches!(
        store.admit(&graph, &next, None).unwrap(),
        ConversationAdmissionOutcome::Existing(_)
    ));
}

#[test]
fn legacy_threads_remain_ownerless_and_cannot_be_reassigned() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("legacy.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let input = ingress(
        "legacy-create",
        person,
        None,
        ConversationIngressAddressing::Unaddressed,
        "Legacy",
    );
    let serialized = serde_json::to_value(&input).unwrap();
    assert!(serialized.get("owner_context_ref").is_none());
    let decoded: ConversationIngress = serde_json::from_value(serialized).unwrap();
    assert_eq!(decoded.owner_context_ref, None);
    let ConversationAdmissionOutcome::Started(started) =
        store.admit(&graph, &decoded, None).unwrap()
    else {
        panic!("new conversation")
    };
    assert_eq!(started.conversation.owner_context_ref, None);
    let mut next = ingress(
        "legacy-move",
        person,
        Some(&started.conversation.conversation_ref),
        ConversationIngressAddressing::Unaddressed,
        "Move",
    );
    next.owner_context_ref = Some("workspace".into());
    assert!(store.admit(&graph, &next, None).is_err());
    next.conversation = ConversationIngressConversation::Create;
    next.owner_context_ref = Some(" ".into());
    assert!(store.admit(&graph, &next, None).is_err());
}

#[test]
fn local_forks_keep_ownership_but_public_imports_do_not() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("fork.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let mut input = ingress(
        "fork-owned",
        person,
        None,
        ConversationIngressAddressing::Unaddressed,
        "Fork me",
    );
    input.owner_context_ref = Some("pack-instance".into());
    let ConversationAdmissionOutcome::Started(started) = store.admit(&graph, &input, None).unwrap()
    else {
        panic!("new conversation")
    };
    let reference = &started.conversation.conversation_ref;
    let message = &started.conversation.messages[0].message_ref;
    let fork = store
        .fork_conversation(&graph, person, reference, message, "owned-fork")
        .unwrap();
    assert_eq!(fork.owner_context_ref, input.owner_context_ref);
    let export = store.export_conversation(reference, person).unwrap();
    let public_json = serde_json::to_value(&export).unwrap();
    assert!(public_json["conversation"]
        .get("owner_context_ref")
        .is_none());
    let imported = store
        .import_conversation(&graph, person, export, None)
        .unwrap();
    assert_eq!(imported.owner_context_ref, None);
    // Even an in-memory or manually constructed transfer cannot move a host-local owner.
    let mut export = store.export_conversation(reference, person).unwrap();
    export.conversation.owner_context_ref = Some("foreign-owner".into());
    let imported = store
        .import_conversation(&graph, person, export, None)
        .unwrap();
    assert_eq!(imported.owner_context_ref, None);
}

#[test]
fn execution_and_host_metadata_preserve_the_stored_owner() {
    let temporary = tempfile::tempdir().unwrap();
    let graph = fixture_graph_with_reviewer();
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("execution.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let mut input = ingress(
        "owned-execution",
        crate::v2026_09::test_fixture::actor_ref("person"),
        None,
        ConversationIngressAddressing::Addressed {
            actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
        },
        "Help",
    );
    input.owner_context_ref = Some("workspace-instance".into());
    let ConversationAdmissionOutcome::Started(started) = store
        .admit(
            &graph,
            &input,
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .unwrap()
    else {
        panic!("new invocation")
    };
    let execution = store
        .execution_conversation(started.record.invocation_ref.as_deref().unwrap())
        .unwrap();
    assert_eq!(execution.owner_context_ref, input.owner_context_ref);
    let metadata = crate::v2026_09::host::project_host_conversation_metadata(
        &store,
        &started.conversation.conversation_ref,
    )
    .unwrap()
    .unwrap();
    assert_eq!(metadata.owner_context_ref, input.owner_context_ref);
    assert!(metadata.messages.is_empty());
}
