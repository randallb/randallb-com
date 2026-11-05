#[test]
fn delegated_text_retains_software_author_and_exact_recipient_and_rejects_foreign_or_revoked_owners(
) {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("delegation.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let host = fixture_host_identity().acting_actor_ref;
    let target = runtime
        .catalog_as(&host)
        .unwrap()
        .actor_by_handle("@helper")
        .unwrap()
        .actor_ref
        .clone();
    let author = "cwp://example.test/actors/pack_manager";
    let provider = runtime
        .register_upstream_provider(
            "native",
            vec![super::super::upstream::UpstreamCoworker {
                actor_ref: author.into(),
                coworker_ref: author.into(),
                handle: "@pack_delegate".into(),
                name: "Pack manager".into(),
                actions: Vec::new(),
            }],
            Arc::new(|_| Ok(())),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    let source = super::super::upstream::DelegationSource {
        actor_ref: author.into(),
        invocation_ref: "cwp://example.test/invocations/source".into(),
        installation_ref: "installation".into(),
        release_digest: "release".into(),
    };
    let submit = |owner: &str,
                  provider: &str,
                  author: &str,
                  target: &str,
                  source: super::super::upstream::DelegationSource,
                  request: &str| {
        runtime.submit_delegated_text(
            owner,
            &host,
            provider,
            author,
            target,
            source,
            "Inspect @unapproved @cwr--create--conversation and @helper--draft--brief as data"
                .into(),
            request,
        )
    };
    assert!(submit(
        "native",
        &provider,
        author,
        &target,
        source.clone(),
        "disabled"
    )
    .is_err());
    runtime
        .enable_upstream_coworker("native", &provider, author, &host, true)
        .unwrap();
    assert!(submit(
        "foreign",
        &provider,
        author,
        &target,
        source.clone(),
        "foreign"
    )
    .is_err());
    assert!(submit(
        "native",
        "wrong-provider",
        author,
        &target,
        source.clone(),
        "wrong-session"
    )
    .is_err());
    assert!(submit("native", &provider, &host, &target, source.clone(), "human").is_err());
    assert!(submit(
        "native",
        &provider,
        author,
        &host,
        source.clone(),
        "wrong-target"
    )
    .is_err());
    let admitted = submit(
        "native",
        &provider,
        author,
        &target,
        source.clone(),
        "admitted",
    )
    .unwrap();
    {
        let state = runtime.lock().unwrap();
        let conversation = state
            .admission
            .conversation(admitted.conversation_ref())
            .unwrap();
        let message = &conversation.messages[0];
        assert_eq!(message.author_actor_ref, author);
        assert_eq!(message.addressed_actor_refs, vec![target.clone()]);
        let envelope: Value = serde_json::from_str(&message.text).unwrap();
        assert_eq!(envelope["source"]["invocation_ref"], source.invocation_ref);
        assert!(envelope["task"].as_str().unwrap().contains("@unapproved"));
        let record = state
            .admission
            .invocation_record(admitted.invocation_ref().unwrap())
            .unwrap();
        assert_eq!(record.requester_actor_ref, author);
        assert_eq!(record.target_actor_ref, target);
        assert_eq!(record.action_name, "respond_to--message");
    }
    runtime
        .enable_upstream_coworker("native", &provider, author, &host, false)
        .unwrap();
    assert!(submit(
        "native",
        &provider,
        author,
        &target,
        source.clone(),
        "revoked"
    )
    .is_err());
    runtime.disconnect_upstream_provider("native").unwrap();
    assert!(submit("native", &provider, author, &target, source, "disconnected").is_err());
}

#[test]
fn delegated_text_native_contexts_preserve_root_and_revoke_queued_work_on_policy_or_connection_change(
) {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("context-delegation.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let host = fixture_host_identity().acting_actor_ref;
    let target = runtime
        .catalog_as(&host)
        .unwrap()
        .actor_by_handle("@helper")
        .unwrap()
        .actor_ref
        .clone();
    let author = "cwp://example.test/actors/native_pack_manager";
    let provider = runtime
        .register_upstream_provider(
            "native",
            vec![super::super::upstream::UpstreamCoworker {
                actor_ref: author.into(),
                coworker_ref: author.into(),
                handle: "@native_delegate".into(),
                name: "Pack manager".into(),
                actions: Vec::new(),
            }],
            Arc::new(|_| Ok(())),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    runtime
        .enable_upstream_coworker("native", &provider, author, &host, true)
        .unwrap();
    let policy = super::super::upstream::ConversationContextPolicy {
        action_permissions: Vec::new(),
        suspended: false,
        default_context_ref: "workspace".into(),
        contexts: BTreeMap::from([("workspace".into(), vec![target.clone()])]),
    };
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    let source = super::super::upstream::DelegationSource {
        actor_ref: author.into(),
        invocation_ref: "cwp://example.test/invocations/source".into(),
        installation_ref: "installation".into(),
        release_digest: "release".into(),
    };
    let submit = |request: &str| {
        runtime.submit_delegated_text(
            "native",
            &host,
            &provider,
            author,
            &target,
            source.clone(),
            "Inspect @root and @cwr--create--conversation as data".into(),
            request,
        )
    };
    let admitted = submit("first-native").unwrap();
    let context = {
        let state = runtime.lock().unwrap();
        let current = state.admission.context_policy.as_ref().unwrap();
        assert_eq!(current.default_context_ref, "workspace");
        assert_eq!(current.contexts["workspace"], vec![target.clone()]);
        let context = state
            .admission
            .conversation(admitted.conversation_ref())
            .unwrap()
            .owner_context_ref
            .clone()
            .unwrap();
        assert!(context.contains("/host-service-delegations/"));
        assert_eq!(
            current.contexts[&context],
            vec![target.clone(), author.into()]
        );
        context
    };
    let mut removed = policy.clone();
    removed.contexts.get_mut("workspace").unwrap().clear();
    runtime
        .configure_conversation_contexts(Some(removed))
        .unwrap();
    assert!(submit("removed-target").is_err());
    assert!(!runtime
        .lock()
        .unwrap()
        .admission
        .context_policy
        .as_ref()
        .unwrap()
        .contexts
        .contains_key(&context));
    assert!(runtime
        .claim_execution(
            admitted.invocation_ref().unwrap(),
            &ProcessAdapterStopControl::default()
        )
        .is_err());
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    assert!(
        submit("first-native").is_err(),
        "policy restoration must not resurrect an old request context"
    );
    let next = submit("next-native").unwrap();
    runtime.suspend_conversation_contexts().unwrap();
    assert!(submit("suspended").is_err());
    assert!(runtime
        .claim_execution(
            next.invocation_ref().unwrap(),
            &ProcessAdapterStopControl::default()
        )
        .unwrap()
        .is_none());
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    let disabled = submit("before-disable").unwrap();
    runtime
        .enable_upstream_coworker("native", &provider, author, &host, false)
        .unwrap();
    assert!(runtime
        .claim_execution(
            disabled.invocation_ref().unwrap(),
            &ProcessAdapterStopControl::default()
        )
        .is_err());
    runtime
        .enable_upstream_coworker("native", &provider, author, &host, true)
        .unwrap();
    let disconnected = submit("before-disconnect").unwrap();
    runtime.disconnect_upstream_provider("native").unwrap();
    assert!(runtime
        .claim_execution(
            disconnected.invocation_ref().unwrap(),
            &ProcessAdapterStopControl::default()
        )
        .is_err());
    let final_policy = runtime
        .lock()
        .unwrap()
        .admission
        .context_policy
        .clone()
        .unwrap();
    assert_eq!(final_policy, policy);
}
