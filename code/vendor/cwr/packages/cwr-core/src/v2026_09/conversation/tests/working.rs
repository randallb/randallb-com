fn working_turn(
    store: &mut ConversationAdmissionStore,
    conversation: Option<&str>,
    event: &str,
) -> AdmittedConversationTurn {
    working_turn_configuration(store, conversation, event, None)
}

fn working_turn_configuration(
    store: &mut ConversationAdmissionStore,
    conversation: Option<&str>,
    event: &str,
    model: Option<&str>,
) -> AdmittedConversationTurn {
    let mut graph = fixture_graph_with_reviewer();
    graph
        .actor_integrations
        .iter_mut()
        .find(|binding| {
            binding.actor_integration_ref == crate::v2026_09::test_fixture::binding_ref("helper")
        })
        .unwrap()
        .adapter_params["selective_history"] = json!(false);
    if let Some(model) = model {
        graph
            .actor_integrations
            .iter_mut()
            .find(|binding| {
                binding.actor_integration_ref
                    == crate::v2026_09::test_fixture::binding_ref("helper")
            })
            .unwrap()
            .adapter_params["model"] = json!(model);
    }
    let mut prepared = store
        .prepare(
            &graph,
            &ingress(
                event,
                crate::v2026_09::test_fixture::actor_ref("person"),
                conversation,
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                event,
            ),
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .unwrap();
    if let ConversationPreparationOutcome::New(turn) = &mut prepared {
        turn.invocation.as_mut().unwrap().conversation_continuity = true;
    }
    let ConversationAdmissionOutcome::Started(turn) = store.commit(prepared).unwrap() else {
        panic!("start")
    };
    *turn
}

#[path = "working/selective.rs"]
mod selective;

#[path = "working/deferred_status.rs"]
mod deferred_status;

#[path = "working/system_store.rs"]
mod system_store;

fn claim_working(
    store: &mut ConversationAdmissionStore,
    turn: &AdmittedConversationTurn,
) -> ClaimedConversationInvocation {
    let ConversationInvocationClaimOutcome::Claimed(claimed) = store
        .claim_invocation(turn.record.invocation_ref.as_deref().unwrap(), 100)
        .unwrap()
    else {
        panic!("claim")
    };
    *claimed
}

fn settle_working(store: &mut ConversationAdmissionStore, claimed: &ClaimedConversationInvocation) {
    let adapter = &claimed.adapter_invocation;
    store.append_execution_activity(&adapter.invocation_ref, ConversationActivityEvent::ProviderEvent {
        provider: "codex".into(), method: "cwr.working_state".into(), data: json!({
            "version":1,"idle":true,"provider":"codex","thread_id":"private-provider-thread",
            "provider_store":"managed",
            "working_ref":crate::adapter::working_context(&adapter.thread).unwrap()["working_conversation"]["ref"],
            "invocation_ref":adapter.invocation_ref,
            "context_snapshot":crate::adapter::working_context(&adapter.thread).unwrap()["context_snapshot"]
        }),
    }).unwrap();
    store
        .complete(&adapter.invocation_ref, json!({"text":"Done"}))
        .unwrap();
}

fn remove_provider_store_from_checkpoint(
    store: &mut ConversationAdmissionStore,
    invocation_ref: &str,
) {
    let execution_ref = store.execution_conversation_ref(invocation_ref).unwrap().to_owned();
    let mut state = store.state.clone();
    let execution = state.conversations.iter_mut()
        .find(|conversation| conversation.conversation_ref == execution_ref).unwrap();
    let checkpoint = execution.activity.iter_mut().find_map(|activity| match &mut activity.event {
        ConversationActivityEvent::ProviderEvent { method, data, .. }
            if method == "cwr.working_state" => Some(data),
        _ => None,
    }).unwrap();
    checkpoint.as_object_mut().unwrap().remove("provider_store");
    store.replace_state(state).unwrap();
}

#[test]
fn old_provider_session_starts_fresh_private_history_after_managed_sign_in() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("legacy-provider.sqlite"), "cwp://com.example.runtime",
    ).unwrap();
    let first = working_turn(&mut store, None, "first request");
    let claimed = claim_working(&mut store, &first);
    settle_working(&mut store, &claimed);
    remove_provider_store_from_checkpoint(&mut store, &claimed.adapter_invocation.invocation_ref);

    let next = working_turn(&mut store, Some(&first.record.conversation_ref), "followup request");
    let followup = claim_working(&mut store, &next);
    let working = &followup.adapter_invocation.thread["working_conversation"];
    assert!(working["history_gap"].as_str().unwrap().contains("sign-in directory changed"));
    assert_eq!(working["previous_invocation_ref"], claimed.adapter_invocation.invocation_ref);
    assert!(working.get("state").is_none());
    assert_ne!(working["ref"], claimed.adapter_invocation.thread["working_conversation"]["ref"]);
    assert_eq!(next.record.conversation_ref, first.record.conversation_ref);
    let public = serde_json::to_string(&followup.adapter_invocation.thread).unwrap();
    assert!(public.contains("first request"));
    assert!(public.contains("followup request"));
}

#[test]
fn private_followup_accepts_codex_versions_without_expanding_other_providers() {
    for (provider, state_provider, version, accepted) in [
        ("codex", "codex", 1, true),
        ("codex", "codex", 2, true),
        ("codex", "codex", 3, true),
        ("other", "other", 1, true),
        ("other", "other", 2, false),
        ("codex", "other", 2, false),
        ("other", "codex", 2, false),
        ("other", "other", 3, false),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("working.sqlite");
        let mut store =
            ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
        let first = working_turn(&mut store, None, "first");
        let claimed = claim_working(&mut store, &first);
        let adapter = &claimed.adapter_invocation;
        let source = crate::adapter::working_context(&adapter.thread).unwrap();
        store
            .append_execution_activity(
                &adapter.invocation_ref,
                ConversationActivityEvent::ProviderEvent {
                    provider: provider.into(),
                    method: "cwr.working_state".into(),
                    data: json!({
                        "version":version,"idle":true,"provider":state_provider,
                        "provider_store":"managed",
                        "thread_id":"private-provider-thread",
                        "working_ref":source["working_conversation"]["ref"],
                        "invocation_ref":adapter.invocation_ref,
                        "context_snapshot":source["context_snapshot"]
                    }),
                },
            )
            .unwrap();
        store
            .complete(&adapter.invocation_ref, json!({"text":"Done"}))
            .unwrap();
        drop(store);
        let mut store =
            ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
        let next = working_turn(&mut store, Some(&first.record.conversation_ref), "followup");
        let result = store.claim_invocation(next.record.invocation_ref.as_deref().unwrap(), 101);
        if accepted {
            let ConversationInvocationClaimOutcome::Claimed(followup) = result.unwrap() else {
                panic!("claim {provider}/{state_provider} version {version}");
            };
            let thread = &followup.adapter_invocation.thread;
            assert_eq!(thread["context_start"], 1);
            assert_eq!(thread["messages"].as_array().unwrap().len(), 2);
            assert_eq!(thread["working_conversation"]["state"]["version"], version);
            assert_eq!(
                thread["working_conversation"]["state"]["thread_id"],
                "private-provider-thread"
            );
        } else {
            let error = result.unwrap_err();
            assert!(
                error.to_string().contains("did not settle safely"),
                "{error}"
            );
        }
    }
}

#[test]
fn private_followup_includes_queued_progress_and_restores_only_the_new_shared_records() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("working.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let first = working_turn(&mut store, None, "first");
    let claimed = claim_working(&mut store, &first);
    let second = working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "queued correction",
    );
    store
        .record_invocation_progress_with_publication(
            &claimed.adapter_invocation.invocation_ref,
            102,
            "Created /tmp/example; installation is still running",
            true,
        )
        .unwrap();
    settle_working(&mut store, &claimed);
    drop(store);
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let followup = claim_working(&mut store, &second);
    let thread = &followup.adapter_invocation.thread;
    assert_eq!(thread["context_start"], 1);
    assert_eq!(thread["context_snapshot"]["message_count"], 4);
    assert_eq!(thread["messages"][0]["text"], "queued correction");
    assert_eq!(thread["messages"][1]["progress"]["status"], 102);
    assert_eq!(
        thread["messages"][1]["author_actor_ref"],
        crate::v2026_09::test_fixture::actor_ref("helper")
    );
    assert_eq!(thread["messages"][2]["text"], "Done");
    assert_eq!(
        thread["working_conversation"]["ref"],
        claimed.adapter_invocation.thread["working_conversation"]["ref"]
    );
    assert_eq!(
        thread["working_conversation"]["previous_invocation_ref"],
        claimed.adapter_invocation.invocation_ref
    );
    store
        .retain_execution_invocation(
            &followup.adapter_invocation.invocation_ref,
            &followup.adapter_invocation,
        )
        .unwrap();
    let retained = retained_context_snapshot(&store, &followup.adapter_invocation.invocation_ref);
    assert!(retained["thread"].get("messages").is_none());
    let third = working_turn(&mut store, Some(&first.record.conversation_ref), "later");
    let restored = context::restore(&store.state, &retained["thread"]).unwrap();
    assert_eq!(restored["messages"], thread["messages"]);
    assert_ne!(
        third.record.execution_conversation_ref,
        second.record.execution_conversation_ref
    );
}

#[test]
fn private_history_is_separate_for_other_conversations_and_cannot_be_claimed_concurrently() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("working.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = working_turn(&mut store, None, "first");
    let claimed = claim_working(&mut store, &first);
    let queued = working_turn(&mut store, Some(&first.record.conversation_ref), "queued");
    let error = store
        .claim_invocation(queued.record.invocation_ref.as_deref().unwrap(), 101)
        .unwrap_err();
    assert!(error.to_string().contains("already working"), "{error}");
    assert_eq!(
        store
            .invocation_record(queued.record.invocation_ref.as_deref().unwrap())
            .unwrap()
            .attempt_count,
        0
    );
    assert!(working::check_child(
        &store.state,
        &AdapterInvocation {
            invocation_ref: "cwp://com.example.runtime/invocations/child".into(),
            ..claimed.adapter_invocation.clone()
        },
        crate::v2026_09::test_fixture::actor_ref("helper"),
        crate::v2026_09::test_fixture::binding_ref("helper")
    )
    .is_err());
    let other = working_turn(&mut store, None, "other");
    let other_claim = claim_working(&mut store, &other);
    assert_ne!(
        other_claim.adapter_invocation.thread["working_conversation"]["ref"],
        claimed.adapter_invocation.thread["working_conversation"]["ref"]
    );
    assert!(
        other_claim.adapter_invocation.thread["working_conversation"]
            .get("state")
            .is_none()
    );
}

#[test]
fn failed_private_history_starts_fresh_and_keeps_published_progress() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("progress.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let first = working_turn(&mut store, None, "first");
    let claimed = claim_working(&mut store, &first);
    let reference = &claimed.adapter_invocation.invocation_ref;
    store
        .record_invocation_progress_with_activity(reference, 100, "Provider is thinking")
        .unwrap();
    let commentary = AdapterCompletedCommentary {
        model_run_ref: claimed.model_run_ref.clone(),
        provider: "codex".into(),
        thread_id: Some("native-thread".into()),
        response_or_turn_id: "turn-1".into(),
        item_id: "message-1".into(),
        text: "The environment exists; startup is unverified.".into(),
    };
    for _ in 0..2 {
        store
            .record_commentary_progress(
                reference,
                claimed.claim.invocation.attempt_count,
                commentary.clone(),
            )
            .unwrap();
    }
    store.recover_interrupted_invocations().unwrap();
    drop(store);
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let messages = &store
        .conversation(&first.record.conversation_ref)
        .unwrap()
        .messages;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].text, commentary.text);
    assert!(messages[1].progress.as_ref().unwrap().status.is_none());
    let next = working_turn(&mut store, Some(&first.record.conversation_ref), "followup");
    let followup = store
        .claim_invocation(next.record.invocation_ref.as_deref().unwrap(), 102)
        .unwrap();
    let ConversationInvocationClaimOutcome::Claimed(followup) = followup else {
        panic!("claim followup")
    };
    let thread = &followup.adapter_invocation.thread;
    assert!(thread["working_conversation"]["history_gap"]
        .as_str()
        .unwrap()
        .contains("previous request failed"));
    assert!(thread["working_conversation"].get("state").is_none());
    assert!(thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["text"] == commentary.text));
    assert!(thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["text"] == "followup"));
}

#[test]
fn stopped_private_history_starts_fresh_without_losing_public_records() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("stopped.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = working_turn(&mut store, None, "first");
    let claimed = claim_working(&mut store, &first);
    store
        .record_invocation_progress_with_publication(
            &claimed.adapter_invocation.invocation_ref,
            102,
            "A completed effect remains visible",
            true,
        )
        .unwrap();
    store
        .stop_invocation(
            &claimed.adapter_invocation.invocation_ref,
            crate::v2026_09::invocation::InvocationStopRequest::new(102),
        )
        .unwrap();
    store
        .settle_stopped_invocation(&claimed.adapter_invocation.invocation_ref, 102)
        .unwrap();

    let next = working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "continue with fresh private history",
    );
    let followup = claim_working(&mut store, &next);
    let thread = &followup.adapter_invocation.thread;
    assert_eq!(
        thread["working_conversation"]["ref"],
        followup.execution_conversation.conversation_ref
    );
    assert_eq!(
        thread["working_conversation"]["previous_invocation_ref"],
        claimed.adapter_invocation.invocation_ref
    );
    assert!(thread["working_conversation"]["history_gap"]
        .as_str()
        .unwrap()
        .contains("previous request was stopped"));
    assert!(thread["working_conversation"].get("state").is_none());
    let messages = thread["messages"].as_array().unwrap();
    assert!(messages
        .iter()
        .any(|message| message["text"] == "A completed effect remains visible"));
    assert!(messages
        .iter()
        .any(|message| message["text"] == "continue with fresh private history"));
}

#[test]
fn private_history_keeps_its_shared_source_when_collaboration_uses_own_conversation() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("working.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = working_turn(&mut store, None, "first");
    let mut claimed = claim_working(&mut store, &first);
    let source = claimed.adapter_invocation.thread.clone();
    claimed.adapter_invocation.execution_scope =
        crate::action_contract::ExecutionScope::OwnConversation;
    claimed.adapter_invocation.thread = conversation_thread(&claimed.execution_conversation);
    claimed.adapter_invocation.thread["requesting_context"] = source;
    store
        .retain_execution_invocation(
            &claimed.adapter_invocation.invocation_ref,
            &claimed.adapter_invocation,
        )
        .unwrap();
    let retained = retained_context_snapshot(&store, &claimed.adapter_invocation.invocation_ref);
    let next = working_turn(&mut store, Some(&first.record.conversation_ref), "next");
    working::initialize(
        &mut store.state,
        &claimed.adapter_invocation.invocation_ref,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        retained_context_snapshot(&store, &claimed.adapter_invocation.invocation_ref),
        retained
    );
    settle_working(&mut store, &claimed);
    let next = claim_working(&mut store, &next);
    assert_eq!(next.adapter_invocation.thread["context_start"], 1);
    assert_eq!(
        next.adapter_invocation.thread["working_conversation"]["previous_invocation_ref"],
        claimed.adapter_invocation.invocation_ref
    );
    assert_eq!(
        next.adapter_invocation.thread["messages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[path = "working/tools.rs"]
mod tools;

include!("working_fingerprints.rs");

#[test]
fn measured_progress_keeps_publication_history_and_projects_latest_key() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("measured.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let turn = working_turn(&mut store, None, "tests");
    let claimed = claim_working(&mut store, &turn);
    let invocation_ref = &claimed.adapter_invocation.invocation_ref;
    for completed in [1, 2] {
        let progress = serde_json::from_value(json!({
            "key":"tests", "phase":"testing", "completed":completed,"total":3,"unit":"binaries"
        }))
        .unwrap();
        let response = store
            .record_measured_invocation_progress(
                invocation_ref,
                102,
                format!("Tests: {completed}/3 complete."),
                true,
                Some(progress),
            )
            .unwrap();
        assert_eq!(response.sequence, completed);
        assert_eq!(response.progress.unwrap().completed, Some(completed));
    }
    let shared = store.conversation(&turn.record.conversation_ref).unwrap();
    let messages: Vec<_> = shared
        .messages
        .iter()
        .filter(|message| message.progress.is_some())
        .collect();
    assert_eq!(messages.len(), 2);
    for message in messages {
        assert_eq!(
            message.author_actor_ref,
            crate::v2026_09::test_fixture::actor_ref("helper")
        );
        assert_eq!(
            message.reply_to_message_ref.as_deref(),
            Some(turn.record.source_message_ref.as_str())
        );
        assert_eq!(
            message.progress.as_ref().unwrap().invocation_ref,
            *invocation_ref
        );
    }
    let projected = crate::v2026_09::host::project_conversation_with_work(
        shared,
        &store.conversations().cloned().collect::<Vec<_>>(),
    );
    let updates = projected
        .activity
        .iter()
        .filter_map(|item| match &item.event {
            ConversationActivityEvent::WorkUpdate {
                text: Some(text), ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!updates.contains(&"Tests: 1/3 complete."));
    assert!(updates.contains(&"Tests: 2/3 complete."));
    settle_working(&mut store, &claimed);
    assert!(store
        .record_measured_invocation_progress(invocation_ref, 102, "Late", true, None)
        .is_err());
    drop(store);
    let store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    let record = store
        .invocation_records()
        .find(|record| record.invocation_ref == *invocation_ref)
        .unwrap();
    assert_eq!(record.responses.len(), 3);
    assert_eq!(
        record.responses[0].progress.as_ref().unwrap().completed,
        Some(1)
    );
    assert_eq!(
        record.responses[1].progress.as_ref().unwrap().completed,
        Some(2)
    );
    assert_eq!(record.responses[2].status.as_u16(), 200);
}
