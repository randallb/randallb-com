#[test]
fn conversation_module_host_commands_preserve_receipts_and_control_authority() {
    let temporary = independent_conversation_fixture();
    let workspace = temporary.path().join("CWR.toml");
    let graph = resolve_local_graph(&workspace).unwrap();
    let person = graph
        .actors
        .iter()
        .find(|a| a.actor_name.as_deref() == Some("Person"))
        .unwrap()
        .actor_ref
        .clone();
    let helper = graph
        .actors
        .iter()
        .find(|a| a.actor_name.as_deref() == Some("Helper"))
        .unwrap()
        .actor_ref
        .clone();
    let ingress = "cwp://example.test/host";
    let path = temporary.path().join("commands.sqlite");
    let runtime = LocalRuntime::open_with_state_path(
        &workspace,
        &path,
        LocalHostIdentity::new(&person, ingress),
    )
    .unwrap();
    let module = runtime.conversation_module().unwrap();
    let text = "@cwr--list--coworkers";
    let admitted = module
        .publish_text_in_context("host-command", None, text, None)
        .unwrap();
    let conversation = admitted.conversation_ref();
    let replay = module.replay_text_in_context("host-command", conversation, text, None);
    // A create receipt cannot be retargeted into a continuation on retry.
    assert!(replay.is_err());
    let continued = module
        .publish_text_in_context("host-followup", Some(conversation), text, None)
        .unwrap();
    let replay = module
        .replay_text_in_context("host-followup", conversation, text, None)
        .unwrap();
    assert_eq!(
        continued.record.source_message_ref,
        replay.record.source_message_ref
    );
    module
        .rename(Some(conversation), "Shared boundary")
        .unwrap();
    let denied = runtime
        .bundled_conversation_module_as(ingress, &helper)
        .unwrap();
    assert!(denied.rename(Some(conversation), "Cannot rename").is_err());
    let ticket = module.reserve_archive(conversation).unwrap();
    let (_, _stopped) = module
        .set_archived_with_stops(Some(conversation), true, Some(ticket))
        .unwrap();
    assert!(
        runtime
            .conversation_host()
            .metadata(conversation)
            .unwrap()
            .unwrap()
            .archived
    );
    module
        .set_archived_with_stops(Some(conversation), false, None)
        .unwrap();
    assert!(
        !runtime
            .conversation_host()
            .metadata(conversation)
            .unwrap()
            .unwrap()
            .archived
    );
}

#[test]
fn conversation_module_rejects_ingress_identity_substitution() {
    let (_temporary, runtime) = participation_runtime();
    let catalog = runtime.catalog().unwrap();
    let actor = catalog.acting_actor.actor_ref;
    let module = runtime
        .bundled_conversation_module_as("cwp://example.test/host", &actor)
        .unwrap();
    let ingress = ConversationIngress {
        owner_context_ref: None,
        file_refs: vec![],
        integration_ref: "cwp://example.test/different-host".into(),
        provider_event_ref: "substitution".into(),
        author_actor_ref: actor,
        conversation: crate::v2026_09::conversation::ConversationIngressConversation::Create,
        addressing: crate::v2026_09::conversation::ConversationIngressAddressing::Unaddressed,
        text: "Cannot substitute ingress".into(),
    };
    assert!(module
        .admit_provider_ingress(ingress, None, None)
        .unwrap_err()
        .to_string()
        .contains("bound conversation caller"));
}

#[test]
fn conversation_module_reserved_archive_stops_provider_work_and_preserves_unrelated_work() {
    let (_temporary, runtime) = participation_runtime();
    let active = claimed_participation_parent(&runtime);
    let conversation = active.conversation_ref.as_deref().unwrap();
    let queued = runtime
        .admit_text(Some(conversation), "@helper queued")
        .unwrap();
    let unrelated = runtime.admit_text(None, "@helper unrelated").unwrap();
    let active_stop = ProcessAdapterStopControl::new();
    let unrelated_stop = ProcessAdapterStopControl::new();
    {
        let mut state = runtime.lock().unwrap();
        state
            .active
            .insert(active.invocation_ref.clone(), active_stop.clone());
        state.active.insert(
            unrelated.invocation_ref().unwrap().into(),
            unrelated_stop.clone(),
        );
    }
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let module = runtime
        .bundled_conversation_module_as("cwp://example.test/host", &person)
        .unwrap();
    let reservation = module.reserve_archive(conversation).unwrap();
    let (_, stopped) = module
        .set_archived_with_stops(Some(conversation), true, Some(reservation))
        .unwrap();
    assert!(active_stop.is_requested());
    assert!(!unrelated_stop.is_requested());
    assert!(stopped.contains(&active.invocation_ref));
    assert!(stopped.contains(&queued.invocation_ref().unwrap().to_owned()));
    module
        .set_archived_with_stops(Some(conversation), false, None)
        .unwrap();
    let mut state = runtime.lock().unwrap();
    for reference in [
        &active.invocation_ref,
        &queued.invocation_ref().unwrap().to_owned(),
    ] {
        assert!(state
            .admission
            .claim_invocation(reference, current_unix_ms())
            .is_ok_and(|result| matches!(
                result,
                ConversationInvocationClaimOutcome::NotRunnable { .. }
            )));
    }
    assert_eq!(
        state
            .admission
            .invocation_record(unrelated.invocation_ref().unwrap())
            .unwrap()
            .state,
        ActionInvocationState::Admitted
    );
}
