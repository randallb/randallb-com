#[test]
fn commentary_identity_is_runtime_scoped_by_provider_integration() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    let ConversationInvocationClaimOutcome::Claimed(root) =
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap()
    else {
        panic!("root invocation must be claimed")
    };
    store
        .record_commentary_progress(
            ROOT_INVOCATION_REF,
            root.claim.invocation.attempt_count,
            commentary(
                &root.model_run_ref,
                "shared-turn",
                "shared-item",
                "Shared provider item",
            ),
        )
        .unwrap();

    let same_integration_ref = "cwp://com.example.runtime/invocations/commentary-same-integration";
    let same_integration = start_claimed_child(
        &mut store,
        same_integration_ref,
        "call-commentary-same-integration",
        "cwp://com.example.fixture/integrations/codex",
        12,
    );
    assert!(matches!(
        store
            .record_commentary_progress(
                same_integration_ref,
                same_integration.claim.invocation.attempt_count,
                commentary(
                    &same_integration.model_run_ref,
                    "shared-turn",
                    "shared-item",
                    "Shared provider item",
                ),
            )
            .unwrap(),
        RecordCommentaryProgressOutcome::Existing(_)
    ));
    assert!(!store
        .execution_conversation(same_integration_ref)
        .unwrap()
        .activity
        .iter()
        .any(|activity| matches!(
            activity.event,
            ConversationActivityEvent::CommentaryProgress { .. }
        )));

    let other_integration_ref =
        "cwp://com.example.runtime/invocations/commentary-other-integration";
    let other_integration = start_claimed_child(
        &mut store,
        other_integration_ref,
        "call-commentary-other-integration",
        "cwp://com.example.fixture/integrations/other-provider-account",
        13,
    );
    assert!(matches!(
        store
            .record_commentary_progress(
                other_integration_ref,
                other_integration.claim.invocation.attempt_count,
                commentary(
                    &other_integration.model_run_ref,
                    "shared-turn",
                    "shared-item",
                    "Shared provider item",
                ),
            )
            .unwrap(),
        RecordCommentaryProgressOutcome::Recorded(_)
    ));
    assert!(store
            .execution_conversation(other_integration_ref)
            .unwrap()
            .activity
            .iter()
            .any(|activity| matches!(
                &activity.event,
                ConversationActivityEvent::CommentaryProgress {
                    provider_integration_ref,
                    ..
                } if provider_integration_ref == "cwp://com.example.fixture/integrations/other-provider-account"
            )));
}

#[test]
fn commentary_replay_and_stale_output_do_not_require_a_state_write() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    let ConversationInvocationClaimOutcome::Claimed(claimed) =
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap()
    else {
        panic!("root invocation must be claimed")
    };
    let attempt_count = claimed.claim.invocation.attempt_count;
    store
        .record_commentary_progress(
            ROOT_INVOCATION_REF,
            attempt_count,
            commentary(&claimed.model_run_ref, "turn-1", "item-1", "Already saved"),
        )
        .unwrap();
    let before = store
        .execution_conversation(ROOT_INVOCATION_REF)
        .unwrap()
        .activity
        .to_vec();
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, true).unwrap();

    assert!(matches!(
        store
            .record_commentary_progress(
                ROOT_INVOCATION_REF,
                attempt_count,
                commentary(&claimed.model_run_ref, "turn-1", "item-1", "Already saved",),
            )
            .unwrap(),
        RecordCommentaryProgressOutcome::Existing(_)
    ));
    assert!(matches!(
        store
            .record_commentary_progress(
                ROOT_INVOCATION_REF,
                attempt_count + 1,
                commentary(
                    &claimed.model_run_ref,
                    "turn-1",
                    "item-stale",
                    "Late output",
                ),
            )
            .unwrap(),
        RecordCommentaryProgressOutcome::IgnoredStaleModelRun
    ));
    assert!(store
        .record_commentary_progress(
            ROOT_INVOCATION_REF,
            attempt_count,
            commentary(
                &claimed.model_run_ref,
                "turn-1",
                "item-new",
                "Needs a write",
            ),
        )
        .is_err());
    assert_eq!(
        store
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .activity,
        before
    );
    cwpdb_store::test_support::commit_faults::reject_runtime_clock_updates(&path, false).unwrap();
}

#[test]
fn commentary_from_an_interrupted_attempt_cannot_reopen_its_result() {
    let temporary = tempfile::tempdir().unwrap();
    let mut store = open(&temporary.path().join("cwpdb.sqlite3"));
    store.commit(prepared_root()).unwrap();
    let ConversationInvocationClaimOutcome::Claimed(claimed) =
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap()
    else {
        panic!("claim root")
    };
    store
        .record_commentary_progress(
            ROOT_INVOCATION_REF,
            claimed.claim.invocation.attempt_count,
            commentary(&claimed.model_run_ref, "turn-1", "item-1", "Saved progress"),
        )
        .unwrap();
    store.recover_interrupted_invocations().unwrap();
    let before = store
        .execution_conversation(ROOT_INVOCATION_REF)
        .unwrap()
        .activity
        .clone();
    let result = store
        .record_commentary_progress(
            ROOT_INVOCATION_REF,
            claimed.claim.invocation.attempt_count,
            commentary(&claimed.model_run_ref, "turn-1", "late-item", "Late output"),
        )
        .unwrap();
    assert_eq!(
        result,
        RecordCommentaryProgressOutcome::IgnoredStaleModelRun
    );
    assert_eq!(
        store
            .execution_conversation(ROOT_INVOCATION_REF)
            .unwrap()
            .activity,
        before
    );
    assert_eq!(
        store.invocation_record(ROOT_INVOCATION_REF).unwrap().state,
        ActionInvocationState::Failed
    );
}

#[test]
fn nested_public_progress_is_retained_once_in_the_shared_root_transcript() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.json");
    let child_ref = "cwp://com.example.runtime/invocations/nested-progress";
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    store
        .transact(|state| {
            state
                .conversations
                .find_mut(|conversation| conversation.conversation_ref == CONVERSATION_REF)
                .unwrap()
                .activity
                .push(ConversationActivity {
                    activity_ref:
                        "cwp://com.example.runtime/activities/root-message-boundary".into(),
                    sequence: 1,
                    occurred_at_unix_ms: 9,
                    event: ConversationActivityEvent::MessageRecorded {
                        message_ref: SOURCE_MESSAGE_REF.into(),
                    },
                });
            Ok(())
        })
        .unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();

    let call = store
        .append_action_call(
            EXECUTION_REF,
            "cwp://com.example.fixture/actors/worker",
            "cwp://com.example.fixture/actors/child",
            "@child--draft--brief",
            json!({"topic": "nested progress"}),
            "@child--draft--brief".into(),
        )
        .unwrap();
    let mut request = child_request(child_ref, "call-nested-progress");
    request.conversation_ref = Some(EXECUTION_REF.into());
    store
        .start_child_invocation(
            request,
            &call.message_ref,
            child_adapter_invocation(child_ref),
        )
        .unwrap();
    let ConversationInvocationClaimOutcome::Claimed(child) =
        store.claim_invocation(child_ref, 21).unwrap()
    else {
        panic!("child invocation must be claimed")
    };

    let commentary = commentary(
        &child.model_run_ref,
        "nested-turn",
        "nested-item",
        "Nested commentary",
    );
    let RecordCommentaryProgressOutcome::Recorded(commentary_activity) = store
        .record_commentary_progress(
            child_ref,
            child.claim.invocation.attempt_count,
            commentary.clone(),
        )
        .unwrap()
    else {
        panic!("new commentary must be recorded")
    };
    store
        .record_invocation_progress_with_publication(
            child_ref,
            102,
            "Nested published progress",
            true,
        )
        .unwrap();
    assert!(matches!(
        store
            .record_commentary_progress(
                child_ref,
                child.claim.invocation.attempt_count,
                commentary,
            )
            .unwrap(),
        RecordCommentaryProgressOutcome::Existing(_)
    ));

    let root = store.conversation(CONVERSATION_REF).unwrap();
    let progress = root
        .messages
        .iter()
        .filter(|message| message.progress.is_some())
        .collect::<Vec<_>>();
    assert_eq!(progress.len(), 2);
    assert_eq!(progress[0].text, "Nested commentary");
    assert_eq!(progress[1].text, "Nested published progress");
    assert_eq!(
        progress[0]
            .progress
            .as_ref()
            .unwrap()
            .source_activity_ref,
        commentary_activity.activity_ref
    );
    for message in &progress {
        assert_eq!(
            message.author_actor_ref,
            "cwp://com.example.fixture/actors/child"
        );
        assert_eq!(
            message.reply_to_message_ref.as_deref(),
            Some(SOURCE_MESSAGE_REF)
        );
        assert_eq!(
            message.progress.as_ref().unwrap().invocation_ref,
            child_ref
        );
    }
    assert!(store
        .conversation(EXECUTION_REF)
        .unwrap()
        .messages
        .iter()
        .all(|message| message.progress.is_none()));
    let observation = store.thread_observation(CONVERSATION_REF).unwrap();
    assert_eq!(
        observation["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["progress"]["invocation_ref"] == child_ref)
            .count(),
        2
    );

    drop(store);
    let mut reopened = open(&path);
    assert_eq!(
        reopened
            .conversation(CONVERSATION_REF)
            .unwrap()
            .messages
            .iter()
            .filter(|message| message
                .progress
                .as_ref()
                .is_some_and(|progress| progress.invocation_ref == child_ref))
            .count(),
        2
    );
    for message in reopened
        .conversation(CONVERSATION_REF)
        .unwrap()
        .messages
        .iter()
        .filter(|message| message.progress.is_some())
    {
        assert_eq!(
            reopened
                .published_progress_conversation_ref(
                    &message.progress.as_ref().unwrap().source_activity_ref
                )
                .unwrap()
                .as_deref(),
            Some(CONVERSATION_REF)
        );
    }

    let export = reopened
        .export_conversation(
            CONVERSATION_REF,
            "cwp://com.example.fixture/actors/requester",
        )
        .unwrap();
    assert_nested_progress_origins(&export.conversation);
    let fork_source_message_ref = export
        .conversation
        .messages
        .iter()
        .rev()
        .find(|message| message.progress.is_some())
        .unwrap()
        .message_ref
        .clone();
    let mut graph = crate::v2026_09::graph::resolve_graph(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/contracts/agent-recovery-turn/workspace/CWR.toml"),
    )
    .unwrap();
    graph.actors[0].actor_ref = "cwp://com.example.fixture/actors/requester".into();
    let fork = reopened
        .fork_conversation(
            &graph,
            "cwp://com.example.fixture/actors/requester",
            CONVERSATION_REF,
            &fork_source_message_ref,
            "nested-progress-fork",
        )
        .unwrap();
    assert_nested_progress_origins(&fork);
}

fn assert_nested_progress_origins(conversation: &RetainedConversation) {
    for message in conversation
        .messages
        .iter()
        .filter(|message| message.progress.is_some())
    {
        let progress = message.progress.as_ref().unwrap();
        let origin = conversation
            .activity
            .iter()
            .find(|activity| activity.activity_ref == progress.source_activity_ref)
            .expect("transferred progress keeps its public work origin");
        assert!(matches!(
            &origin.event,
            ConversationActivityEvent::WorkUpdate {
                invocation_ref,
                source_message_ref,
                actor_ref,
                status,
                ..
            } if invocation_ref == &progress.invocation_ref
                && Some(source_message_ref) == message.reply_to_message_ref.as_ref()
                && actor_ref == &message.author_actor_ref
                && progress.status.is_none_or(|retained| retained == *status)
        ));
        assert!(conversation.activity.iter().any(|activity| matches!(
            &activity.event,
            ConversationActivityEvent::MessageWorkAssociation {
                message_ref,
                activity_ref,
            } if message_ref == &message.message_ref
                && activity_ref == &progress.source_activity_ref
        )));
    }
}
