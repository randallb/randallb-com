#[test]
fn standalone_action_recovery_preserves_retained_ownership() {
    let (_temporary, runtime) = participation_runtime();
    let person = participation_ref(&runtime, "person");
    let integration = "cwp://example.test/owned-action";
    let activation = "@cwr--list--actions";
    let original = runtime
        .admit_action_in_upstream_scope(
            integration,
            "root",
            &person,
            None,
            activation,
            json!({}),
            None,
        )
        .unwrap();
    // Simulate a trusted host stamping ownership on standalone ingress. The first
    // recovery must reconstruct that persisted owner, not the current navigation.
    {
        let mut state = runtime.lock().unwrap();
        let graph = state.graph.clone();
        let ingress = ConversationIngress {
            owner_context_ref: Some("workspace".into()),
            file_refs: vec![],
            integration_ref: integration.into(),
            provider_event_ref: "owned-root".into(),
            author_actor_ref: person.clone(),
            conversation: ConversationIngressConversation::CreateForAction,
            addressing: ConversationIngressAddressing::Action {
                actor_ref: original.record.target_actor_ref.unwrap(),
                activation: activation.into(),
                action_name: "list--actions".into(),
                input: json!({}),
            },
            text: activation.into(),
        };
        state
            .admission
            .admit(
                &graph,
                &ingress,
                original.record.actor_integration_ref.as_deref(),
            )
            .unwrap();
    }
    let recovered = runtime
        .recover_action_request(
            integration,
            "owned-root",
            &person,
            None,
            activation,
            &json!({}),
            None,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered.conversation.owner_context_ref.as_deref(),
        Some("workspace")
    );
}

// Characterization for the pack-owned schedules spike. This deliberately asserts
// the current gap, not the desired scheduler behavior. Remove/replace it when
// ordinary standalone action admission gains an explicit installed owner.
#[test]
fn scheduled_action_boundary_currently_selects_workspace_instead_of_installed_owner() {
    use crate::v2026_09::upstream::ConversationContextPolicy;
    let (_temporary, runtime) = participation_runtime();
    let person = participation_ref(&runtime, "person");
    let target = add_participation_actor(&runtime, "schedule_fixture");
    mark_actor_ready_for_test(&runtime, &target);
    runtime
        .configure_conversation_contexts(Some(ConversationContextPolicy {
            action_permissions: Vec::new(),
            suspended: false,
            default_context_ref: "workspace".into(),
            contexts: BTreeMap::from([
                ("workspace".into(), vec![target.clone()]),
                ("installation-a".into(), vec![target.clone()]),
                ("installation-b".into(), vec![target]),
            ]),
        }))
        .unwrap();
    let admitted = runtime
        .admit_action_in_upstream_scope(
            "cwp://example.test/schedule-probe",
            "occurrence-a",
            &person,
            None,
            "@schedule_fixture--draft--brief",
            json!({}),
            None,
        )
        .unwrap();
    assert_eq!(
        admitted.conversation.owner_context_ref.as_deref(),
        Some("workspace")
    );
    let retry = runtime
        .admit_action_in_upstream_scope(
            "cwp://example.test/schedule-probe",
            "occurrence-a",
            &person,
            None,
            "@schedule_fixture--draft--brief",
            json!({}),
            None,
        )
        .unwrap();
    assert_eq!(retry.record, admitted.record);
}

#[test]
fn scheduled_action_boundary_pack_only_target_needs_an_owned_admission_operation() {
    use crate::v2026_09::upstream::ConversationContextPolicy;
    let (_temporary, runtime) = participation_runtime();
    let person = participation_ref(&runtime, "person");
    let target = add_participation_actor(&runtime, "schedule_fixture");
    mark_actor_ready_for_test(&runtime, &target);
    runtime
        .configure_conversation_contexts(Some(ConversationContextPolicy {
            action_permissions: Vec::new(),
            suspended: false,
            default_context_ref: "workspace".into(),
            contexts: BTreeMap::from([
                ("workspace".into(), vec![]),
                ("installation-a".into(), vec![target.clone()]),
                ("installation-b".into(), vec![target.clone()]),
            ]),
        }))
        .unwrap();
    assert!(runtime
        .admit_action_in_upstream_scope(
            "cwp://example.test/schedule-probe",
            "unowned-occurrence",
            &person,
            None,
            "@schedule_fixture--draft--brief",
            json!({}),
            None,
        )
        .is_err());
    // The internal ingress primitive can represent the required owner. This is
    // evidence for a narrow owning operation, not an application bypass.
    let route = runtime
        .graph()
        .unwrap()
        .routes
        .into_iter()
        .find(|route| route.actor_ref == target && route.action_name == "draft--brief")
        .unwrap();
    let ingress = ConversationIngress {
        owner_context_ref: Some("installation-a".into()),
        file_refs: vec![],
        integration_ref: "cwp://example.test/schedule-probe".into(),
        provider_event_ref: "owned-occurrence-a".into(),
        author_actor_ref: person,
        conversation: ConversationIngressConversation::CreateForAction,
        addressing: ConversationIngressAddressing::Action {
            actor_ref: target,
            activation: "@schedule_fixture--draft--brief".into(),
            action_name: "draft--brief".into(),
            input: json!({}),
        },
        text: "@schedule_fixture--draft--brief".into(),
    };
    let admitted = runtime
        .admit_ingress(ingress.clone(), Some(&route.actor_integration_ref))
        .unwrap();
    assert_eq!(
        admitted.conversation.owner_context_ref.as_deref(),
        Some("installation-a")
    );
    let retry = runtime
        .admit_ingress(ingress.clone(), Some(&route.actor_integration_ref))
        .unwrap();
    assert_eq!(retry.record, admitted.record);
    let mut changed = ingress;
    changed.owner_context_ref = Some("installation-b".into());
    assert!(runtime
        .admit_ingress(changed, Some(&route.actor_integration_ref))
        .is_err());
}

#[test]
fn owned_standalone_action_preserves_exact_owner_and_receipt_after_target_removal() {
    use crate::v2026_09::upstream::ConversationContextPolicy;
    let (temporary, runtime) = participation_runtime();
    let person = participation_ref(&runtime, "person");
    let actor = add_participation_actor(&runtime, "owned_schedule_fixture");
    mark_actor_ready_for_test(&runtime, &actor);
    let owner = "cwp://example.test/installations/a";
    let policy = ConversationContextPolicy {
        action_permissions: vec![],
        suspended: false,
        default_context_ref: "cwp://example.test/workspace".into(),
        contexts: BTreeMap::from([
            ("cwp://example.test/workspace".into(), vec![]),
            (owner.into(), vec![actor.clone()]),
            (
                "cwp://example.test/installations/b".into(),
                vec![actor.clone()],
            ),
        ]),
    };
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    let route = runtime
        .graph()
        .unwrap()
        .routes
        .into_iter()
        .find(|r| r.actor_ref == actor && r.action_name == "draft--brief")
        .unwrap();
    let target = route_action_target(&runtime.graph().unwrap().runtime_ref, &route);
    let context = runtime
        .installed_action_context_as(owner, &person, "cwp://example.test/schedule-ingress")
        .unwrap();
    let first = context.admit("occurrence-1", &target, json!({})).unwrap();
    assert_eq!(first.conversation.owner_context_ref.as_deref(), Some(owner));
    assert!(first.conversation.is_sidebar_eligible());
    assert_eq!(first.conversation.messages.len(), 1);
    assert_eq!(
        context
            .admit("occurrence-1", &target, json!({}))
            .unwrap()
            .record,
        first.record
    );
    assert!(context
        .admit("occurrence-1", &target, json!({"changed":true}))
        .is_err());
    for field in ["role", "binding", "contract", "runtime", "actor"] {
        let mut changed = target.clone();
        match field {
            "role" => changed.role_ref.push_str("-changed"),
            "binding" => changed.binding_ref.push_str("-changed"),
            "contract" => changed.contract_id.push_str("-changed"),
            "runtime" => changed.runtime_ref.push_str("-changed"),
            _ => changed.coworker_ref.push_str("-changed"),
        }
        assert!(context.admit("occurrence-1", &changed, json!({})).is_err());
        assert!(context
            .admit(&format!("new-{field}"), &changed, json!({}))
            .is_err());
    }
    let other = runtime
        .installed_action_context_as(
            "cwp://example.test/installations/b",
            &person,
            "cwp://example.test/schedule-ingress",
        )
        .unwrap();
    assert!(other.admit("occurrence-1", &target, json!({})).is_err());
    runtime
        .lock()
        .unwrap()
        .graph
        .routes
        .retain(|r| r.actor_ref != actor);
    assert_eq!(
        context
            .admit("occurrence-1", &target, json!({}))
            .unwrap()
            .record,
        first.record
    );
    assert!(context.admit("occurrence-2", &target, json!({})).is_err());
    drop(runtime);
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    // Restart loads a suspended context policy and no longer has the synthetic
    // target. Receipt recovery must still return the committed invocation.
    let context = runtime
        .installed_action_context_as(owner, &person, "cwp://example.test/schedule-ingress")
        .unwrap();
    assert_eq!(
        context
            .admit("occurrence-1", &target, json!({}))
            .unwrap()
            .record,
        first.record
    );
    assert!(context.admit("occurrence-3", &target, json!({})).is_err());
    assert!(runtime
        .installed_action_context_as("", &person, "cwp://example.test/schedule-ingress")
        .is_err());
}

#[test]
fn native_owner_actions_require_current_exact_authority_and_stay_out_of_conversation_lists() {
    use crate::v2026_09::upstream::{ContextActionPermission, ConversationContextPolicy};
    let (_temporary, runtime) = participation_runtime();
    let person = participation_ref(&runtime, "person");
    let actor = add_participation_actor(&runtime, "native_owner_fixture");
    mark_actor_ready_for_test(&runtime, &actor);
    let ingress = "cwp://example.test/native-owner-ingress";
    let owner = "urn:uuid:installed-native-owner";
    let other = "urn:uuid:other-native-owner";
    let activation = "@native_owner_fixture--draft--brief";
    let admit = |request: &str, destination: &str, input| {
        runtime.admit_owner_action_in_upstream_scope(
            ingress,
            request,
            &person,
            destination,
            activation,
            input,
            None,
        )
    };
    assert!(admit("unconfigured", owner, json!({})).is_err());
    let policy = ConversationContextPolicy {
        action_permissions: Vec::new(),
        suspended: false,
        default_context_ref: "workspace".into(),
        contexts: BTreeMap::from([
            ("workspace".into(), Vec::new()),
            (owner.into(), vec![actor.clone()]),
            (other.into(), vec![actor.clone()]),
            ("wrong-owner".into(), Vec::new()),
        ]),
    };
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    for destination in ["workspace", "wrong-owner", "missing-owner"] {
        assert!(admit(destination, destination, json!({})).is_err());
    }
    assert!(runtime
        .admit_owner_action_in_upstream_scope(
            ingress,
            "unknown-person",
            "cwp://example.test/unknown-person",
            owner,
            activation,
            json!({}),
            None,
        )
        .is_err());
    let first = admit("native-1", owner, json!({})).unwrap();
    assert_eq!(first.conversation.owner_context_ref.as_deref(), Some(owner));
    assert_eq!(
        first.record.target_actor_ref.as_deref(),
        Some(actor.as_str())
    );
    assert!(!first.conversation.is_sidebar_eligible());
    assert_eq!(first.conversation.sidebar_eligible, Some(false));
    assert_eq!(
        admit("native-1", owner, json!({})).unwrap().record,
        first.record
    );
    let before_replay_denial = runtime.conversations().unwrap().len();
    assert!(runtime
        .admit_action_as(ingress, "native-1", &person, None, activation, json!({}))
        .is_err());
    assert_eq!(runtime.conversations().unwrap().len(), before_replay_denial);
    assert!(admit("native-1", other, json!({})).is_err());
    assert_eq!(runtime.conversations().unwrap().len(), before_replay_denial);
    assert!(admit("native-1", owner, json!({"changed":true})).is_err());
    assert!(runtime
        .admit_action_as(
            ingress,
            "workspace-default",
            &person,
            None,
            activation,
            json!({})
        )
        .is_err());

    // A current action with changed schemas cannot replay an old exact contract.
    let original = runtime.graph().unwrap();
    runtime
        .lock()
        .unwrap()
        .graph
        .routes
        .iter_mut()
        .find(|route| route.actor_ref == actor && route.action_name == "draft--brief")
        .unwrap()
        .output_schema["description"] = json!("changed contract");
    assert!(admit("native-1", owner, json!({})).is_err());
    runtime.lock().unwrap().graph = original;

    let mut suspended = policy.clone();
    suspended.suspended = true;
    runtime
        .configure_conversation_contexts(Some(suspended))
        .unwrap();
    assert!(admit("native-1", owner, json!({})).is_err());
    assert!(admit("native-2", owner, json!({})).is_err());
    let mut disabled = policy;
    disabled.contexts.insert(owner.into(), Vec::new());
    // A cross-context grant never replaces membership of this owner destination.
    disabled.action_permissions.push(ContextActionPermission {
        caller_actor_ref: person.clone(),
        target_actor_ref: actor,
        action_names: vec!["draft--brief".into()],
    });
    runtime
        .configure_conversation_contexts(Some(disabled))
        .unwrap();
    assert!(admit("native-1", owner, json!({})).is_err());
    assert!(admit("native-2", owner, json!({})).is_err());
    let retained = runtime.conversations().unwrap();
    assert_eq!(retained.len(), 2, "one action root and its execution child");
    assert_eq!(
        retained
            .iter()
            .filter(|conversation| conversation.parent_conversation_ref.is_none())
            .count(),
        1
    );
    assert!(retained.iter().any(|conversation| {
        Some(conversation.conversation_ref.as_str())
            == first.record.execution_conversation_ref.as_deref()
            && conversation.parent_conversation_ref.as_deref()
                == Some(first.record.conversation_ref.as_str())
    }));
    assert_eq!(
        runtime
            .conversations()
            .unwrap()
            .iter()
            .filter(|conversation| conversation.is_sidebar_eligible())
            .count(),
        0
    );
}
