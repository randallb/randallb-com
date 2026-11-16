#[test]
fn availability_restart_waits_for_current_policy_before_scheduling() {
    let (temporary, runtime) = participation_runtime();
    let helper = participation_ref(&runtime, "helper");
    let policy = crate::v2026_09::upstream::ConversationContextPolicy {
        action_permissions: Vec::new(),
        suspended: false,
        default_context_ref: "workspace".into(),
        contexts: BTreeMap::from([("workspace".into(), vec![helper])]),
    };
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    let first = runtime.admit_text(None, "@helper First").unwrap();
    drop(runtime);
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    assert!(runtime.try_schedule_execution().unwrap().is_none());
    assert_eq!(
        runtime
            .lock()
            .unwrap()
            .admission
            .invocation_record(first.invocation_ref().unwrap())
            .unwrap()
            .attempt_count,
        0
    );
    runtime
        .configure_conversation_contexts(Some(policy))
        .unwrap();
    let task = runtime
        .try_schedule_execution()
        .unwrap()
        .expect("fresh policy releases pending work");
    assert_eq!(
        task.record().invocation_ref,
        first.invocation_ref().unwrap()
    );
}

#[test]
fn availability_commit_rejects_prepared_work_after_policy_change() {
    let (_temporary, runtime) = participation_runtime();
    let helper = participation_ref(&runtime, "helper");
    let person = participation_ref(&runtime, "person");
    let mut policy = crate::v2026_09::upstream::ConversationContextPolicy {
        action_permissions: Vec::new(),
        suspended: false,
        default_context_ref: "workspace".into(),
        contexts: BTreeMap::from([("workspace".into(), vec![helper.clone()])]),
    };
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    let prepared = {
        let state = runtime.lock().unwrap();
        state
            .admission
            .prepare(
                &state.graph,
                &ConversationIngress {
                    owner_context_ref: Some("workspace".into()),
                    file_refs: vec![],
                    integration_ref: "cwp://example.test/prepared".into(),
                    provider_event_ref: "prepared".into(),
                    author_actor_ref: person,
                    conversation: ConversationIngressConversation::Create,
                    addressing: ConversationIngressAddressing::Addressed {
                        actor_refs: vec![helper],
                    },
                    text: "Prepared".into(),
                },
                None,
            )
            .unwrap()
    };
    policy.contexts.get_mut("workspace").unwrap().clear();
    runtime
        .configure_conversation_contexts(Some(policy))
        .unwrap();
    let mut state = runtime.lock().unwrap();
    assert!(
        state
            .admission
            .commit(prepared)
            .unwrap_err()
            .is::<ConversationContextError>()
    );
    assert_eq!(state.admission.admissions().count(), 0);
    assert_eq!(state.admission.invocation_records().count(), 0);
}

#[test]
fn availability_nested_work_checks_caller_and_destination_before_readiness() {
    for revoke_caller in [false, true] {
        let (_temporary, runtime) = participation_runtime();
        let reviewer = add_participation_actor(&runtime, "reviewer");
        let caller = claimed_participation_parent(&runtime);
        let members = if revoke_caller {
            vec![reviewer.clone()]
        } else {
            vec![caller.target_actor_ref.clone()]
        };
        runtime
            .configure_conversation_contexts(Some(
                crate::v2026_09::upstream::ConversationContextPolicy {
                    action_permissions: Vec::new(),
                    suspended: false,
                    default_context_ref: "workspace".into(),
                    contexts: BTreeMap::from([("workspace".into(), members)]),
                },
            ))
            .unwrap();
        let target = {
            let state = runtime.lock().unwrap();
            let route = state
                .graph
                .routes
                .iter()
                .find(|route| route.actor_ref == reviewer)
                .unwrap();
            exact_action_target(&state, route)
        };
        let message = AdapterCall {
            invocation_ref: caller.invocation_ref.clone(),
            call_id: "message".into(),
            operation: SEND_MESSAGE_HOST_OPERATION.into(),
            input: json!({"to":reviewer,"text":"blocked"}),
        };
        let input = SendMessageInput {
            runtime_ref: None,
            binding_ref: None,
            to: reviewer,
            text: "blocked".into(),
        };
        assert!(
            runtime
                .message_preflight_with_readiness(&input, &message, &caller, |_| panic!(
                    "denied message must not probe readiness"
                ))
                .err()
                .unwrap()
                .is::<ConversationContextError>()
        );
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        for (id, input) in [
            (
                "named",
                json!({"activation":"@reviewer--draft--brief", "input":{}}),
            ),
            ("exact", json!({"target":target,"input":{}})),
        ] {
            let call = AdapterCall {
                invocation_ref: caller.invocation_ref.clone(),
                call_id: id.into(),
                operation: START_ACTION_HOST_OPERATION.into(),
                input,
            };
            assert!(
                runtime
                    .handle_start_action(&call, &caller, &queue)
                    .unwrap_err()
                    .is::<ConversationContextError>()
            );
        }
        // Gateway endpoints are not context-member destinations. Their caller
        // policy and executor availability use a configured gateway fixture in
        // availability_gateway_checks_caller_before_executor_availability.
        assert!(queue.borrow().is_empty());
        assert_eq!(
            runtime
                .lock()
                .unwrap()
                .admission
                .invocation_records()
                .count(),
            1
        );
    }
}

#[test]
fn availability_preserves_approved_gateway_transport() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = gateway_test_workspace(temp.path());
    let person = load_default_local_acting_actor_ref(&manifest).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        &manifest,
        temp.path().join("state.sqlite"),
        LocalHostIdentity::new(&person, "cwp://example.test/host"),
    )
    .unwrap();
    let helper = participation_ref(&runtime, "helper");
    runtime
        .configure_conversation_contexts(Some(
            crate::v2026_09::upstream::ConversationContextPolicy {
                action_permissions: Vec::new(),
                suspended: false,
                default_context_ref: "workspace".into(),
                contexts: BTreeMap::from([("workspace".into(), vec![helper])]),
            },
        ))
        .unwrap();
    let initial = runtime.admit_text(None, "@helper hello").unwrap();
    {
        let mut state = runtime.lock().unwrap();
        let gateway = &mut state.graph.gateways[0];
        gateway.connection_ref = Some("test-connection".into());
        gateway.ingress_route = Some(gateway.route.clone());
        state.admission.response_graph = Some(state.graph.clone());
    }
    let entry = runtime.admit_gateway_action_in_upstream_scope("test-connection", "cwp://example.test/host", "gateway-entry", &person, Some(initial.conversation_ref()), "gateway--handle--request", json!({"request":{},"ref":"request","requester_actor_ref":person,"parent_invocation_ref":"parent"}), None).unwrap();
    assert!(entry.invocation_ref().is_some());
    assert_eq!(
        entry.conversation.owner_context_ref.as_deref(),
        Some("workspace")
    );
}

#[test]
fn availability_denied_root_work_never_probes_provider_readiness() {
    let (_temporary, runtime) = participation_runtime();
    let helper = participation_ref(&runtime, "helper");
    let person = participation_ref(&runtime, "person");
    let binding = {
        let mut state = runtime.lock().unwrap();
        let actor = state
            .graph
            .actors
            .iter_mut()
            .find(|actor| actor.actor_ref == helper)
            .unwrap();
        let binding = actor.default_actor_integration_ref.clone().unwrap();
        actor.message_binding_preference = vec![binding.clone()];
        binding
    };
    runtime
        .configure_conversation_contexts(Some(
            crate::v2026_09::upstream::ConversationContextPolicy {
                action_permissions: Vec::new(),
                suspended: false,
                default_context_ref: "workspace".into(),
                contexts: BTreeMap::from([("workspace".into(), vec![])]),
            },
        ))
        .unwrap();
    for exact in [None, Some(binding.as_str())] {
        let ingress = ConversationIngress {
            owner_context_ref: Some("workspace".into()),
            file_refs: vec![],
            integration_ref: "cwp://example.test/denied".into(),
            provider_event_ref: format!("{exact:?}"),
            author_actor_ref: person.clone(),
            conversation: ConversationIngressConversation::Create,
            addressing: ConversationIngressAddressing::Addressed {
                actor_refs: vec![helper.clone()],
            },
            text: "Denied".into(),
        };
        assert!(
            runtime
                .admit_ingress_with_readiness(ingress, exact, None, |_| panic!(
                    "denied root must not probe readiness"
                ))
                .unwrap_err()
                .is::<ConversationContextError>()
        );
    }
    assert_eq!(runtime.lock().unwrap().admission.admissions().count(), 0);
}

#[test]
fn availability_cleared_policy_stays_revoked_for_owned_threads_after_restart() {
    let (temporary, runtime) = participation_runtime();
    runtime.configure_conversation_contexts(None).unwrap();
    assert!(!runtime.lock().unwrap().admission.context_policy_configured);
    assert_eq!(
        runtime
            .conversation_context_targets(Some("opaque-owner"))
            .unwrap(),
        None
    );
    let person = participation_ref(&runtime, "person");
    let helper = participation_ref(&runtime, "helper");
    runtime
        .configure_conversation_contexts(Some(
            crate::v2026_09::upstream::ConversationContextPolicy {
                action_permissions: Vec::new(),
                suspended: false,
                default_context_ref: "workspace".into(),
                contexts: BTreeMap::from([("workspace".into(), vec![helper])]),
            },
        ))
        .unwrap();
    let first = runtime.admit_text(None, "@helper First").unwrap();
    runtime.configure_conversation_contexts(None).unwrap();
    drop(runtime);
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    assert!(
        runtime
            .submit_text_as(
                "cwp://example.test/clear",
                "blocked",
                &person,
                Some(first.conversation_ref()),
                "@helper Continue"
            )
            .unwrap_err()
            .is::<ConversationContextError>()
    );
    runtime
        .submit_text_as(
            "cwp://example.test/clear",
            "control",
            &person,
            Some(first.conversation_ref()),
            "@cwr--show--help",
        )
        .unwrap();
    assert!(
        runtime
            .admit_text(None, "@helper Legacy")
            .unwrap()
            .conversation
            .owner_context_ref
            .is_none()
    );
}

#[test]
fn availability_returns_conversational_replies_to_the_original_person() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = person_workspace(temp.path(), true);
    let person = load_default_local_acting_actor_ref(&workspace).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        &workspace,
        temp.path().join("state.sqlite"),
        LocalHostIdentity::new(&person, "cwp://example.test/replies"),
    )
    .unwrap();
    let helper = participation_ref(&runtime, "helper");
    runtime
        .configure_conversation_contexts(Some(
            crate::v2026_09::upstream::ConversationContextPolicy {
                action_permissions: Vec::new(),
                suspended: false,
                default_context_ref: "workspace".into(),
                contexts: BTreeMap::from([("workspace".into(), vec![helper.clone()])]),
            },
        ))
        .unwrap();
    let first = runtime.admit_text(None, "@helper Ask me").unwrap();
    runtime
        .lock()
        .unwrap()
        .admission
        .complete(
            first.invocation_ref().unwrap(),
            json!({"text":"Which color?"}),
        )
        .unwrap();
    let question = runtime
        .lock()
        .unwrap()
        .admission
        .invocation_records()
        .find(|record| record.state == ActionInvocationState::AwaitingPerson)
        .cloned()
        .unwrap();
    assert_eq!(question.target_actor_ref, person);
    runtime
        .reply_person_request(
            &person,
            first.conversation_ref(),
            &question.invocation_ref,
            "Blue".into(),
        )
        .unwrap();
    let state = runtime.lock().unwrap();
    assert_eq!(
        state
            .admission
            .invocation_records()
            .find(|record| record.is_runnable())
            .unwrap()
            .target_actor_ref,
        helper
    );
}

#[test]
fn scoped_discovery_preserves_identity_of_numeric_ref_participant() {
    let (_temporary, runtime) = participation_runtime();
    let person = "cwp://org.example/resources/123abc";
    {
        let mut state = runtime.lock().unwrap();
        let actor = state.graph.actors.iter_mut()
            .find(|actor| actor.coworker_slug == "person").unwrap();
        actor.actor_ref = person.into();
        actor.coworker_ref = person.into();
    }
    let admission = runtime.submit_text_as(
        "cwp://example.test/numeric-person", "first", person, None, "@helper First",
    ).unwrap();
    let helper = participation_ref(&runtime, "helper");
    runtime.configure_conversation_contexts(Some(
        crate::v2026_09::upstream::ConversationContextPolicy {
            action_permissions: Vec::new(),
            suspended: false,
            default_context_ref: "workspace".into(),
            contexts: BTreeMap::from([("workspace".into(), vec![helper.clone()])]),
        },
    )).unwrap();
    let response = runtime.discovery_as(
        Some(admission.conversation_ref()), &helper, None,
        &IntrinsicCwrCommand::ListCoworkers,
    ).unwrap().unwrap();
    let value = serde_json::to_value(response).unwrap();
    let text = value.to_string();
    assert!(text.contains("@person"), "{text}");
    assert!(text.contains("123abc"), "{text}");
}

#[test]
fn scoped_host_transport_availability_allows_named_dispatch_only_in_its_manager_context() {
    let (_temporary, runtime) = participation_runtime();
    let endpoint = add_participation_actor(&runtime, "service_endpoint");
    let caller = claimed_participation_parent(&runtime);
    let target = {
        let state = runtime.lock().unwrap();
        exact_action_target(&state, state.graph.routes.iter().find(|route| route.actor_ref == endpoint).unwrap())
    };
    let policy = |permit| crate::v2026_09::upstream::ConversationContextPolicy {
        action_permissions: Vec::new(),
        suspended: false,
        default_context_ref: "workspace".into(),
        contexts: BTreeMap::from([
            ("workspace".into(), if permit { vec![caller.target_actor_ref.clone(), endpoint.clone()] } else { vec![caller.target_actor_ref.clone()] }),
            ("foreign-pack".into(), vec![caller.target_actor_ref.clone()]),
        ]),
    };
    runtime.configure_conversation_contexts(Some(policy(true))).unwrap();
    let queue = Rc::new(RefCell::new(VecDeque::new()));
    let call = AdapterCall { invocation_ref: caller.invocation_ref.clone(), call_id: "approved-service".into(), operation: INVOKE_ACTION_HOST_OPERATION.into(), input: json!({"target":target,"input":{}}) };
    runtime.handle_start_action(&call, &caller, &queue).unwrap();
    let child = queue.borrow_mut().pop_front().unwrap();
    let record = runtime.lock().unwrap().admission.invocation_record(&child).unwrap().clone();
    assert_eq!(record.target_actor_ref, endpoint);
    let mut foreign_policy = policy(true);
    foreign_policy.default_context_ref = "foreign-pack".into();
    runtime.configure_conversation_contexts(Some(foreign_policy)).unwrap();
    let foreign_caller = claimed_participation_parent(&runtime);
    let foreign_call = AdapterCall { invocation_ref: foreign_caller.invocation_ref.clone(), call_id: "foreign-service".into(), ..call.clone() };
    assert!(runtime.handle_start_action(&foreign_call, &foreign_caller, &queue).unwrap_err().is::<ConversationContextError>());
    runtime.configure_conversation_contexts(Some(policy(false))).unwrap();
    let denied = AdapterCall { call_id: "revoked-service".into(), ..call };
    assert!(runtime.handle_start_action(&denied, &caller, &queue).unwrap_err().is::<ConversationContextError>());
    assert!(queue.borrow().is_empty());
}
