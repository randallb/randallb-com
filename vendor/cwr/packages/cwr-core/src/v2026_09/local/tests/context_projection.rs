#[test]
#[expect(
    clippy::too_many_lines,
    reason = "One persistent runtime scenario covers admission, pagination, and reopening the same owned histories."
)]
fn conversation_contexts_preserve_ownership_and_filter_before_pagination() {
    use crate::v2026_09::upstream::ConversationContextPolicy;
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("contexts.sqlite");
    let author = crate::v2026_09::test_fixture::actor_ref("person");
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let integration = "cwp://example.test/integrations/contexts";
    let workspace = "cwp://example.test/contexts/workspace";
    let pack = "cwp://example.test/contexts/pack";
    let disabled = "cwp://example.test/contexts/disabled";
    let policy = ConversationContextPolicy {
        action_permissions: Vec::new(),
        suspended: false,
        default_context_ref: workspace.into(),
        contexts: BTreeMap::from([
            (workspace.into(), vec![author.into(), helper.into()]),
            (pack.into(), vec![helper.into()]),
            (disabled.into(), vec![]),
        ]),
    };
    let runtime =
        LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity())
            .unwrap();
    let legacy = runtime
        .submit_text_as(integration, "legacy", author, None, "Legacy")
        .unwrap();
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    let first = runtime
        .submit_message_in_context(
            integration,
            "first",
            author,
            None,
            "Workspace brief",
            vec![],
            Some("host"),
            Some(workspace),
        )
        .unwrap();
    assert_eq!(
        first.conversation.owner_context_ref.as_deref(),
        Some(workspace)
    );
    assert_eq!(
        runtime
            .host_conversation_metadata(&first.conversation.conversation_ref)
            .unwrap()
            .unwrap()
            .owner_context_ref
            .as_deref(),
        Some(workspace)
    );
    let control = runtime
        .submit_message_in_context(
            integration,
            "intrinsic-help",
            author,
            Some(&first.conversation.conversation_ref),
            "@cwr--show--help",
            vec![],
            Some("host"),
            None,
        )
        .unwrap();
    assert_eq!(
        control.record.target_actor_ref.as_deref(),
        Some(crate::action_contract::INTRINSIC_CWR_ACTOR_REF)
    );
    let retry = runtime
        .submit_message_in_context(
            integration,
            "first",
            author,
            None,
            "Workspace brief",
            vec![],
            Some("host"),
            Some(workspace),
        )
        .unwrap();
    assert_eq!(
        retry.conversation.conversation_ref,
        first.conversation.conversation_ref
    );
    assert!(
        runtime
            .submit_message_in_context(
                integration,
                "first",
                author,
                None,
                "Workspace brief",
                vec![],
                Some("host"),
                Some(pack)
            )
            .is_err()
    );
    assert!(
        runtime
            .submit_message_in_context(
                integration,
                "blocked",
                author,
                None,
                "@helper blocked",
                vec![],
                Some("host"),
                Some(disabled)
            )
            .is_err()
    );
    assert!(
        runtime
            .admission_record(integration, "blocked")
            .unwrap()
            .is_none()
    );
    let mut owned = Vec::new();
    for index in 0..3 {
        let result = runtime
            .submit_message_in_context(
                integration,
                format!("pack-{index}"),
                author,
                None,
                format!("@helper Pack {index}"),
                vec![],
                Some("host"),
                Some(pack),
            )
            .unwrap();
        owned.push(result.conversation.conversation_ref);
    }
    assert!(
        runtime
            .submit_message_in_context(
                integration,
                "move",
                author,
                Some(&owned[0]),
                "@helper do not move",
                vec![],
                Some("host"),
                Some(workspace)
            )
            .is_err()
    );
    let continued = runtime
        .submit_message_in_context(
            integration,
            "continue",
            author,
            Some(&owned[0]),
            "@helper Continue",
            vec![],
            Some("host"),
            None,
        )
        .unwrap();
    assert_eq!(
        continued.conversation.owner_context_ref.as_deref(),
        Some(pack)
    );
    drop(runtime);
    let runtime =
        LocalRuntime::open_with_state_path(fixture_workspace(), &path, fixture_host_identity())
            .unwrap();
    runtime
        .configure_conversation_contexts(Some(policy.clone()))
        .unwrap();
    assert_eq!(
        runtime.conversation_context_targets(Some(pack)).unwrap(),
        Some(policy.contexts[pack].clone())
    );
    let snapshot = runtime
        .publication_snapshot_until(
            author,
            Some("host"),
            Instant::now() + Duration::from_secs(5),
            || false,
            |_| {},
        )
        .unwrap();
    assert_eq!(
        snapshot.owner_context_ref(&owned[0]).unwrap().as_deref(),
        Some(pack)
    );
    let page = snapshot
        .conversations(
            Some(false),
            &ConversationSummaryPageRequest {
                owner_context_ref: Some(pack.into()),
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(page.total_count, 3);
    assert_eq!(page.summaries.len(), 1);
    assert!(page.has_next_page);
    assert_eq!(page.summaries[0].key.conversation_ref, owned[2]);
    let request = ConversationSummaryPageRequest {
        owner_context_ref: Some(pack.into()),
        first: Some(1),
        ..Default::default()
    };
    let direct = runtime
        .root_conversation_summaries(
            &request,
            Instant::now() + Duration::from_secs(5),
            || false,
            |_| {},
        )
        .unwrap();
    assert_eq!(direct.total_count, 3);
    assert_eq!(direct.summaries.len(), 1);
    assert_eq!(direct.summaries[0].key, page.summaries[0].key);
    let participant = runtime
        .participant_root_conversation_summaries(
            author,
            &request,
            Some(false),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(participant.total_count, 3);
    assert_eq!(participant.summaries[0].key, page.summaries[0].key);
    let second = snapshot
        .conversations(
            Some(false),
            &ConversationSummaryPageRequest {
                owner_context_ref: Some(pack.into()),
                first: Some(2),
                after: Some(page.summaries[0].key.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(second.total_count, 3);
    assert_eq!(second.summaries.len(), 2);
    let workspace_page = snapshot
        .conversations(
            None,
            &ConversationSummaryPageRequest {
                owner_context_ref: Some(workspace.into()),
                include_legacy_owner: true,
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(workspace_page.total_count, 2);
    assert!(
        workspace_page
            .summaries
            .iter()
            .any(|item| item.key.conversation_ref == legacy.conversation.conversation_ref)
    );
    assert!(
        snapshot
            .conversations(
                None,
                &ConversationSummaryPageRequest {
                    owner_context_ref: Some(workspace.into()),
                    after: Some(page.summaries[0].key.clone()),
                    first: Some(1),
                    ..Default::default()
                }
            )
            .is_err()
    );
}
