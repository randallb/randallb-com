#[test]
fn shared_conversation_preserves_terminal_scope_for_people_and_model_children() {
    let (_directory, runtime, alice_tui, _alice_session, _receiver) = fixture();
    let alice = crate::v2026_09::test_fixture::actor_ref("person");
    let bob = crate::v2026_09::test_fixture::actor_ref("helper");
    let mut bob_tui = alice_tui.clone();
    bob_tui.actor_ref = "cwp://example.test/tuis/bob/actors/tui".into();
    let bob_session = runtime
        .register_upstream_provider(
            "bob-connection",
            vec![bob_tui.clone()],
            Arc::new(|_| Ok(())),
            std::sync::mpsc::sync_channel(1).0,
        )
        .unwrap();
    runtime
        .enable_upstream_coworker(
            "bob-connection",
            &bob_session,
            &bob_tui.actor_ref,
            bob,
            true,
        )
        .unwrap();

    // An accepted request makes both configured actors participants, while its
    // model invocation retains the originating terminal connection.
    let parent = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "shared-seed",
            alice,
            None,
            "@helper coordinate",
            Some("connection"),
        )
        .unwrap();
    assert!(parent
        .conversation
        .participant_actor_refs
        .contains(&alice.to_owned()));
    assert!(parent
        .conversation
        .participant_actor_refs
        .contains(&bob.to_owned()));
    for (author, owner, expected, foreign) in [
        (alice, "connection", &alice_tui, &bob_tui),
        (bob, "bob-connection", &bob_tui, &alice_tui),
    ] {
        let catalog = runtime.upstream_catalog_as(author, Some(owner)).unwrap();
        assert_eq!(
            catalog.actor_by_handle("@tui").unwrap().actor_ref,
            expected.actor_ref
        );
        assert!(catalog.actor_by_ref(&foreign.actor_ref).is_none());
        let own = runtime
            .submit_text_in_upstream_scope(
                "cwp://example.test/ingress",
                format!("own-{owner}"),
                author,
                Some(parent.conversation_ref()),
                "@tui--new--conversation",
                Some(owner),
            )
            .unwrap();
        let state = runtime.lock().unwrap();
        assert_eq!(
            retained_upstream_route(&state, own.invocation_ref().unwrap())
                .unwrap()
                .coworker
                .actor_ref,
            expected.actor_ref
        );
        drop(state);
        let forged = runtime.admit_ingress_in_scope(
            ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://example.test/ingress".into(),
                provider_event_ref: format!("foreign-{owner}"),
                author_actor_ref: author.into(),
                conversation: ConversationIngressConversation::Continue {
                    conversation_ref: parent.conversation_ref().into(),
                },
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec![foreign.actor_ref.clone()],
                },
                text: "new".into(),
            },
            None,
            Some(owner),
        );
        assert!(
            forged.is_err(),
            "knowing the other terminal's actor ref cannot grant access"
        );
    }

    let caller = {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claimed) = state
            .admission
            .claim_invocation(parent.invocation_ref().unwrap(), 10)
            .unwrap()
        else {
            panic!("expected model claim");
        };
        claimed.claim.invocation
    };
    // Use a valid exact target discovered in Bob's scope, rather than a malformed
    // target whose rejection would say nothing about cross-terminal authority.
    let foreign_target = {
        let state = runtime.lock().unwrap();
        let graph = upstream_graph(&state, Some("bob-connection")).unwrap();
        let route = graph
            .routes
            .iter()
            .find(|route| route.actor_ref == bob_tui.actor_ref)
            .unwrap();
        exact_action_target(&state, route)
    };
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    let forged = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "model-cross-terminal".into(),
        operation: START_ACTION_HOST_OPERATION.into(),
        input: json!({"target":foreign_target,"input":{}}),
    };
    assert!(runtime
        .handle_start_action(&forged, &caller, &queue)
        .is_err());
    assert!(queue.borrow().is_empty());
    let own = AdapterCall {
        invocation_ref: caller.invocation_ref.clone(),
        call_id: "model-own-terminal".into(),
        operation: START_ACTION_HOST_OPERATION.into(),
        input: json!({"activation":"@tui--new--conversation","input":{}}),
    };
    let AdapterHostCallOutcome::Returned(result) =
        runtime.handle_start_action(&own, &caller, &queue).unwrap();
    let child: StartActionResult = serde_json::from_value(result).unwrap();
    let state = runtime.lock().unwrap();
    assert_eq!(
        retained_upstream_route(&state, &child.invocation_ref)
            .unwrap()
            .coworker
            .actor_ref,
        alice_tui.actor_ref
    );
}
