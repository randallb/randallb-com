fn fixture_graph_with_action() -> ResolvedCwrGraph {
    let mut graph = fixture_graph_with_reviewer();
    let integration = graph
        .actor_integrations
        .iter()
        .find(|integration| integration.actor_ref == crate::v2026_09::test_fixture::actor_ref("helper"))
        .expect("helper actor integration")
        .clone();
    graph
        .routes
        .push(crate::v2026_09::graph::ResolvedCwrActionRoute {
            role_ref: crate::v2026_09::test_fixture::role_ref("helper").into(),
            contract_revision: "1".into(),
                execution_scope: Default::default(),
                response_mode: Default::default(),
            actor_ref: integration.actor_ref,
            coworker_ref: integration.coworker_ref,
            coworker_source: integration.coworker_source,
            working_path: integration.working_path,
            action_name: "draft--brief".into(),
            action_title: "Draft brief".into(),
            action_description: "Draft a brief.".into(),
            input_schema: json!({"type": "object", "additionalProperties": false}),
            output_schema: json!({"type": "object"}),
            actor_integration_ref: integration.actor_integration_ref,
            actor_integration_source: integration.actor_integration_source,
            integration_ref: integration.integration_ref,
            integration_source: integration.integration_source,
            adapter_ref: integration.adapter_ref,
            available_authority: integration.available_authority,
            missing_required_authority: integration.missing_required_authority,
            adapter_params: integration.adapter_params,
            integration_params: integration.integration_params,
            actor_integration_params: integration.actor_integration_params,
            input_uses: integration.input_uses,
            input_plan: integration.input_plan,
        });
    graph
}

#[test]
fn action_target_is_inherited_by_following_message() {
    let temporary = tempfile::tempdir().expect("temporary state");
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("conversation.json"),
        "cwp://com.example.runtime",
    )
    .expect("open store");
    let graph = fixture_graph_with_action();
    let action = store
        .admit(
            &graph,
            &ingress(
                "event-action",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                ConversationIngressAddressing::Action {
                    actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
                    activation: "@helper--draft--brief".into(),
                    action_name: "draft--brief".into(),
                    input: json!({}),
                },
                "@helper--draft--brief",
            ),
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .expect("admit action");
    let ConversationAdmissionOutcome::Started(action) = action else {
        panic!("action must start a conversation")
    };

    let continued = store
        .admit(
            &graph,
            &ingress(
                "event-continued",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&action.record.conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                "continue",
            ),
            None,
        )
        .expect("inherit action target");
    let ConversationAdmissionOutcome::Started(continued) = continued else {
        panic!("continued admission must start a turn")
    };
    assert_eq!(
        continued.record.target_actor_ref.as_deref(),
        Some(crate::v2026_09::test_fixture::actor_ref("helper"))
    );
    assert!(continued
        .conversation
        .messages
        .last()
        .unwrap()
        .action
        .is_none());
}

#[test]
fn create_for_action_rejects_non_action_ingress_without_retaining_state() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        temporary.path().join("invalid-action-create.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let graph = fixture_graph_with_action();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let invalid_addressing = [
        ConversationIngressAddressing::Unaddressed,
        ConversationIngressAddressing::Addressed {
            actor_refs: vec![helper.into()],
        },
    ];

    for (index, addressing) in invalid_addressing.into_iter().enumerate() {
        let mut request = ingress(
            &format!("invalid-action-create-{index}"),
            person,
            None,
            addressing,
            "ordinary message",
        );
        request.conversation = ConversationIngressConversation::CreateForAction;
        let error = store.admit(&graph, &request, None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("requires action addressing"),
            "{error}"
        );
    }

    assert_eq!(store.conversations().count(), 0);
    assert_eq!(store.admissions().count(), 0);
    assert_eq!(store.invocation_records().count(), 0);
}

#[test]
fn action_replay_accepts_only_the_exact_persisted_create_fingerprint() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("legacy-action.sqlite");
    let graph = fixture_graph_with_action();
    let binding = Some(crate::v2026_09::test_fixture::binding_ref("helper"));
    let original = ingress(
        "legacy-action", crate::v2026_09::test_fixture::actor_ref("person"), None,
        ConversationIngressAddressing::Action {
            actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
            activation: "@helper--draft--brief".into(),
            action_name: "draft--brief".into(), input: json!({}),
        }, "@helper--draft--brief",
    );
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let ConversationAdmissionOutcome::Started(first) = store.admit(&graph, &original, binding).unwrap()
        else { panic!("expected initial admission") };
    assert_eq!(first.record.ingress_fingerprint.as_deref(), Some(conversation_ingress_fingerprint(&original).unwrap().as_str()));
    // Reproduce the pre-marker retained shape, with the original fingerprint intact.
    store.transact(|state| {
        state.conversations.find_mut(|conversation| conversation.conversation_ref == first.record.conversation_ref)
            .unwrap().sidebar_eligible = None;
        Ok(())
    }).unwrap();
    drop(store);
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let legacy = store.conversation(&first.record.conversation_ref).unwrap();
    assert!(legacy.messages[0].action.is_some());
    assert!(legacy.is_sidebar_eligible());
    let mut execution = legacy.clone();
    execution.parent_conversation_ref = Some(legacy.conversation_ref.clone());
    assert!(!execution.is_sidebar_eligible());

    let mut upgraded = original.clone();
    upgraded.conversation = ConversationIngressConversation::CreateForAction;
    assert_ne!(conversation_ingress_fingerprint(&upgraded).unwrap(), first.record.ingress_fingerprint.clone().unwrap());
    let ConversationAdmissionOutcome::Existing(replayed) = store.admit(&graph, &upgraded, binding).unwrap()
        else { panic!("expected the persisted action admission") };
    assert_eq!(*replayed, first.record);
    for change in 0..4 {
        let mut changed = upgraded.clone();
        match change {
            0 => changed.text.push_str(" changed"),
            1 => {
                let ConversationIngressAddressing::Action { input, .. } = &mut changed.addressing else { unreachable!() };
                *input = json!({"changed": true});
            }
            2 => {
                let ConversationIngressAddressing::Action { activation, .. } = &mut changed.addressing else { unreachable!() };
                activation.push_str("-changed");
            }
            _ => changed.author_actor_ref = "cwp://com.example.fixture/actors/reviewer".into(),
        }
        let error = store.admit(&graph, &changed, binding).unwrap_err();
        assert!(error.to_string().contains("reused with different input"), "{error}");
    }
    assert_eq!(store.conversations().count(), 2);
    assert_eq!(store.invocation_records().count(), 1);
}
