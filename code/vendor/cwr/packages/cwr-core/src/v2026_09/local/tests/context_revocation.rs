#[test]
fn context_policy_is_rechecked_after_provider_readiness() {
    let (_temporary, runtime) = participation_runtime();
    let helper = participation_ref(&runtime, "helper");
    let person = participation_ref(&runtime, "person");
    let binding = runtime
        .graph()
        .unwrap()
        .actors
        .iter()
        .find(|actor| actor.actor_ref == helper)
        .unwrap()
        .default_actor_integration_ref
        .clone()
        .unwrap();
    runtime
        .lock()
        .unwrap()
        .graph
        .actors
        .iter_mut()
        .find(|actor| actor.actor_ref == helper)
        .unwrap()
        .message_binding_preference = vec![binding];
    let policy = crate::v2026_09::upstream::ConversationContextPolicy {
        action_permissions: Vec::new(),
        suspended: false,
        default_context_ref: "workspace".into(),
        contexts: BTreeMap::from([("workspace".into(), vec![helper.clone()])]),
    };
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    let ingress = ConversationIngress {
        owner_context_ref: Some("workspace".into()),
        file_refs: vec![],
        integration_ref: "cwp://example.test/context-race".into(),
        provider_event_ref: "race".into(),
        author_actor_ref: person,
        conversation: ConversationIngressConversation::Create,
        addressing: ConversationIngressAddressing::Addressed {
            actor_refs: vec![helper],
        },
        text: "race".into(),
    };
    let mut probes = 0;
    let result = runtime.admit_ingress_with_readiness(ingress, None, None, |_| {
        probes += 1;
        let mut removed = policy.clone();
        removed.contexts.get_mut("workspace").unwrap().clear();
        runtime
            .configure_conversation_contexts(Some(removed))
            .unwrap();
        AdapterReadiness {
            status: crate::adapter::AdapterReadinessStatus::Ready,
            code: "ready".into(),
            message: "ready".into(),
            repair: None,
        }
    });
    assert_eq!(probes, 1);
    assert!(result.unwrap_err().is::<ConversationContextError>());
    assert_eq!(runtime.lock().unwrap().admission.admissions().count(), 0);
}

#[test]
fn delayed_person_reply_cannot_restart_a_coworker_removed_from_its_context() {
    for remove_composition in [false, true] {
        use crate::v2026_09::upstream::ConversationContextPolicy;
        let temporary = tempfile::tempdir().unwrap();
        let workspace = person_workspace(temporary.path(), true);
        let person = load_default_local_acting_actor_ref(&workspace).unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            &workspace,
            temporary.path().join("state.sqlite"),
            LocalHostIdentity::new(&person, "cwp://example.test/context-replies"),
        )
        .unwrap();
        let helper = participation_ref(&runtime, "helper");
        let policy = |include_helper| ConversationContextPolicy {
            action_permissions: Vec::new(),
            suspended: false,
            default_context_ref: "workspace".into(),
            contexts: BTreeMap::from([
                (
                    "workspace".into(),
                    if include_helper {
                        vec![person.clone(), helper.clone()]
                    } else {
                        vec![person.clone()]
                    },
                ),
                ("pack".into(), vec![helper.clone()]),
            ]),
        };
        runtime
            .configure_conversation_contexts(Some(policy(true)))
            .unwrap();
        let question = runtime
            .submit_message_in_context(
                "cwp://example.test/context-replies",
                "question",
                &helper,
                None,
                "@person Which color?",
                vec![],
                Some("host"),
                Some("workspace"),
            )
            .unwrap();
        runtime
            .configure_conversation_contexts(Some(policy(false)))
            .unwrap();
        {
            // Exercise the transaction check directly, as if policy changed after reply preflight.
            let mut state = runtime.lock().unwrap();
            let error = state
                .admission
                .publish_root_message_completed(
                    question.invocation_ref().unwrap(),
                    ActionResponseStatus::COMPLETED,
                    json!({"text":"Blue"}),
                    "Replied".into(),
                    current_unix_ms(),
                )
                .unwrap_err();
            assert!(error.is::<ConversationContextError>(), "{error}");
            assert_eq!(
                state
                    .admission
                    .invocation_record(question.invocation_ref().unwrap())
                    .unwrap()
                    .state,
                ActionInvocationState::AwaitingPerson
            );
        }
        assert!(
            runtime
                .reply_person_request_in_upstream_scope(
                    &person,
                    question.conversation_ref(),
                    question.invocation_ref().unwrap(),
                    "Blue".into(),
                    Some("host")
                )
                .unwrap_err()
                .is::<ConversationContextError>()
        );
        assert_eq!(
            runtime
                .conversation(question.conversation_ref())
                .unwrap()
                .unwrap()
                .messages
                .len(),
            1
        );
        if remove_composition {
            runtime.configure_conversation_contexts(None).unwrap();
            assert!(
                runtime
                    .reply_person_request_in_upstream_scope(
                        &person,
                        question.conversation_ref(),
                        question.invocation_ref().unwrap(),
                        "Blue".into(),
                        Some("host")
                    )
                    .is_err()
            );
        }
        runtime
            .configure_conversation_contexts(Some(policy(true)))
            .unwrap();
        runtime
            .reply_person_request_in_upstream_scope(
                &person,
                question.conversation_ref(),
                question.invocation_ref().unwrap(),
                "Blue".into(),
                Some("host"),
            )
            .unwrap();
        let state = runtime.lock().unwrap();
        let followup = state
            .admission
            .invocation_records()
            .find(|record| record.target_actor_ref == helper && record.is_runnable())
            .unwrap();
        assert_eq!(
            state.admission.working_conversation_ref(followup).unwrap(),
            question.conversation_ref()
        );
    }
}

#[test]
fn removed_context_keeps_history_and_intrinsic_controls_without_coworker_admission() {
    let (_temporary, runtime) = participation_runtime();
    let helper = participation_ref(&runtime, "helper");
    let person = participation_ref(&runtime, "person");
    runtime
        .configure_conversation_contexts(Some(
            crate::v2026_09::upstream::ConversationContextPolicy {
                action_permissions: Vec::new(),
                suspended: false,
                default_context_ref: "pack".into(),
                contexts: BTreeMap::from([("pack".into(), vec![helper])]),
            },
        ))
        .unwrap();
    let first = runtime
        .submit_message_in_context(
            "cwp://example.test/removed-context",
            "first",
            &person,
            None,
            "@helper Draft",
            vec![],
            None,
            Some("pack"),
        )
        .unwrap();
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
    assert!(
        runtime
            .submit_message_in_context(
                "cwp://example.test/removed-context",
                "unknown-owner",
                &person,
                None,
                "@cwr--show--help",
                vec![],
                None,
                Some("unknown-pack")
            )
            .unwrap_err()
            .is::<ConversationContextError>()
    );
    assert_eq!(
        runtime.conversation_context_targets(Some("pack")).unwrap(),
        Some(vec![])
    );
    assert!(
        runtime
            .conversation(first.conversation_ref())
            .unwrap()
            .is_some()
    );
    assert!(
        runtime
            .submit_text_as(
                "cwp://example.test/removed-context",
                "blocked",
                &person,
                Some(first.conversation_ref()),
                "@helper Continue"
            )
            .unwrap_err()
            .is::<ConversationContextError>()
    );
    let control = runtime
        .submit_text_as(
            "cwp://example.test/removed-context",
            "control",
            &person,
            Some(first.conversation_ref()),
            "@cwr--show--help",
        )
        .unwrap();
    assert_eq!(
        control.record.target_actor_ref.as_deref(),
        Some(INTRINSIC_CWR_ACTOR_REF)
    );
    assert!(
        runtime
            .discovery_as(
                Some(first.conversation_ref()),
                &person,
                None,
                &IntrinsicCwrCommand::ShowHelp
            )
            .unwrap()
            .is_some()
    );
}

#[test]
fn suspended_contexts_deny_admission_and_stay_closed_after_restart() {
    let (temporary, runtime) = participation_runtime();
    let person = participation_ref(&runtime, "person");
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
    let first = runtime
        .submit_text_as(
            "cwp://example.test/suspended",
            "first",
            &person,
            None,
            "@helper First",
        )
        .unwrap();
    runtime.suspend_conversation_contexts().unwrap();
    assert_eq!(
        runtime.default_conversation_context().unwrap().as_deref(),
        Some("workspace")
    );
    assert!(
        runtime
            .submit_text_as(
                "cwp://example.test/suspended",
                "blocked",
                &person,
                None,
                "Write something"
            )
            .unwrap_err()
            .is::<ConversationContextError>()
    );
    assert_eq!(
        runtime.conversation_context_targets(None).unwrap(),
        Some(vec![])
    );
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
                "cwp://example.test/suspended",
                "restart-blocked",
                &person,
                Some(first.conversation_ref()),
                "@helper Continue"
            )
            .unwrap_err()
            .is::<ConversationContextError>()
    );
    let control = runtime
        .submit_text_as(
            "cwp://example.test/suspended",
            "control",
            &person,
            Some(first.conversation_ref()),
            "@cwr--show--help",
        )
        .unwrap();
    assert_eq!(
        control.record.target_actor_ref.as_deref(),
        Some(INTRINSIC_CWR_ACTOR_REF)
    );
    runtime
        .configure_conversation_contexts(Some(policy))
        .unwrap();
    assert!(
        runtime
            .submit_text_as(
                "cwp://example.test/suspended",
                "resumed",
                &person,
                Some(first.conversation_ref()),
                "@helper Continue"
            )
            .is_ok()
    );
}

#[test]
fn provider_questions_respect_revocation_but_recover_existing_requests() {
    for revoke in ["remove", "suspend", "member"] {
        let temp = tempfile::tempdir().unwrap();
        let workspace = person_workspace(temp.path(), true);
        let person = load_default_local_acting_actor_ref(&workspace).unwrap();
        let runtime = LocalRuntime::open_with_state_path(
            &workspace,
            temp.path().join("state.sqlite"),
            LocalHostIdentity::new(&person, "cwp://example.test/questions"),
        )
        .unwrap();
        let helper = participation_ref(&runtime, "helper");
        let mut policy = crate::v2026_09::upstream::ConversationContextPolicy {
            action_permissions: Vec::new(),
            suspended: false,
            default_context_ref: "pack".into(),
            contexts: BTreeMap::from([("pack".into(), vec![helper]), ("workspace".into(), vec![])]),
        };
        runtime
            .configure_conversation_contexts(Some(policy.clone()))
            .unwrap();
        let root = runtime.admit_text(None, "@helper ask a question").unwrap();
        let caller = {
            let mut state = runtime.lock().unwrap();
            let ConversationInvocationClaimOutcome::Claimed(claim) = state
                .admission
                .claim_invocation(root.invocation_ref().unwrap(), 10)
                .unwrap()
            else {
                panic!("claim")
            };
            claim.claim.invocation
        };
        assert_eq!(
            provider_question_call(&runtime, &caller, "existing", "start").unwrap()["state"],
            "pending"
        );
        match revoke {
            "remove" => {
                policy.contexts.remove("pack");
                policy.default_context_ref = "workspace".into();
            }
            "suspend" => policy.suspended = true,
            _ => policy.contexts.get_mut("pack").unwrap().clear(),
        }
        runtime
            .configure_conversation_contexts(Some(policy))
            .unwrap();
        let before = runtime.lock().unwrap().admission.admissions().count();
        assert!(
            provider_question_call(&runtime, &caller, "new", "start")
                .unwrap_err()
                .is::<ConversationContextError>()
        );
        assert_eq!(
            runtime.lock().unwrap().admission.admissions().count(),
            before
        );
        assert_eq!(
            provider_question_call(&runtime, &caller, "existing", "start").unwrap()["state"],
            "pending"
        );
        assert_eq!(
            provider_question_call(&runtime, &caller, "existing", "poll").unwrap()["state"],
            "pending"
        );
        assert_eq!(
            provider_question_call(&runtime, &caller, "existing", "cancel").unwrap()["state"],
            "resolved"
        );
    }
}

#[test]
fn adapter_action_discovery_hides_out_of_context_targets() {
    let (_temporary, runtime) = participation_runtime();
    let reviewer = add_participation_actor(&runtime, "reviewer");
    mark_actor_ready_for_test(&runtime, &reviewer);
    let caller = claimed_participation_parent(&runtime);
    // The root was created in legacy mode; it belongs to the default context.
    let mut policy = crate::v2026_09::upstream::ConversationContextPolicy {
        action_permissions: Vec::new(),
        suspended: false,
        default_context_ref: "workspace".into(),
        contexts: BTreeMap::from([("workspace".into(), vec![caller.target_actor_ref.clone()])]),
    };
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    let hidden = list_actions_for(&runtime, &caller, json!({})).unwrap();
    assert!(
        !hidden["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["target"]["coworker_ref"] == reviewer)
    );
    assert!(list_actions_for(&runtime, &caller, json!({"coworker_ref":reviewer})).is_err());
    policy
        .contexts
        .get_mut("workspace")
        .unwrap()
        .push(reviewer.clone());
    runtime
        .configure_conversation_contexts(Some(policy))
        .unwrap();
    let visible = list_actions_for(&runtime, &caller, json!({"coworker_ref":reviewer})).unwrap();
    assert_eq!(visible["actions"][0]["target"]["coworker_ref"], reviewer);
}


include!("source_action_permissions.rs");
include!("private_message_permissions.rs");
