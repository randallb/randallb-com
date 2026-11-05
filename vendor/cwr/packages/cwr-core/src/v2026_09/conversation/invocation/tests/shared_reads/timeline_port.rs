use super::*;

#[test]
fn finite_timeline_preserves_indexed_dependencies_windows_and_revision_fences() {
    for rollout in [
        ConversationTimelineRollout::Disabled,
        ConversationTimelineRollout::NewConversations,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pages.sqlite");
        let mut store =
            ConversationAdmissionStore::open_with_timeline_rollout(&path, RUNTIME_ROOT, rollout)
                .unwrap();
        store.commit(prepared_root()).unwrap();
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
        let child = "cwp://com.example.runtime/invocations/pages-child";
        start_claimed_child(
            &mut store,
            child,
            "pages-child",
            "cwp://com.example.fixture/integrations/codex",
            25,
        );
        store
            .publish_child_action_terminal(
                child,
                InvocationTerminal {
                    status: ActionResponseStatus::COMPLETED,
                    summary: "Child".into(),
                    result: Some(json!({"text":"Child reply"})),
                    error: None,
                },
                "Child reply".into(),
                40,
            )
            .unwrap();
        store
            .publish_root_message_completed(
                ROOT_INVOCATION_REF,
                ActionResponseStatus::COMPLETED,
                json!({"text":"Root reply"}),
                "Root".into(),
                50,
            )
            .unwrap();
        let compare = |request: &ConversationRecordPageRequest| {
            let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
            let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
            let expected = operation.conversation_timeline(CONVERSATION_REF, request.window());
            let actual = store.canonical_timeline_page(
                CONVERSATION_REF,
                request,
                None,
                false,
                &cwpdb_store::conversation::ReadControl {
                    cancellation: cwpdb_store::conversation::ReadCancellation::default(),
                    deadline: std::time::Instant::now() + Duration::from_secs(5),
                    vm_steps: None,
                },
            );
            match (actual, expected) {
                (Ok(actual), Ok(expected)) => {
                    assert_eq!(actual, expected, "window {request:?} in {rollout:?}");
                    Some(actual)
                }
                (Err(_), Err(_)) => None,
                (actual, expected) => panic!("port/indexed disagree: {actual:?} / {expected:?}"),
            }
        };
        let (all, steps) = crate::v2026_09::conversation::read::step_measurement::measure(|| {
            compare(&ConversationRecordPageRequest {
                first: Some(500),
                ..Default::default()
            })
        });
        assert!(
            steps > 0,
            "finite timeline must measure its native storage work"
        );
        let all = all.unwrap();
        assert!(all.anchors.len() >= 3);
        let anchor_key = |a: &ConversationTimelineAnchor| match a {
            ConversationTimelineAnchor::Message { key, .. }
            | ConversationTimelineAnchor::Turn { key, .. } => key.clone(),
        };
        let first = anchor_key(all.anchors.first().unwrap());
        let last = anchor_key(all.anchors.last().unwrap());
        for request in [
            ConversationRecordPageRequest {
                first: Some(0),
                ..Default::default()
            },
            ConversationRecordPageRequest {
                last: Some(0),
                ..Default::default()
            },
            ConversationRecordPageRequest {
                first: Some(2),
                ..Default::default()
            },
            ConversationRecordPageRequest {
                last: Some(2),
                ..Default::default()
            },
            ConversationRecordPageRequest {
                first: Some(2),
                last: Some(1),
                ..Default::default()
            },
            ConversationRecordPageRequest {
                first: Some(0),
                last: Some(2),
                ..Default::default()
            },
            ConversationRecordPageRequest {
                after: Some(first.clone()),
                first: Some(2),
                ..Default::default()
            },
            ConversationRecordPageRequest {
                before: Some(last.clone()),
                last: Some(2),
                ..Default::default()
            },
            ConversationRecordPageRequest {
                after: Some(first.clone()),
                before: Some(last.clone()),
                first: Some(500),
                last: Some(0),
            },
            ConversationRecordPageRequest {
                after: Some(ConversationPositionKey {
                    position: 9999,
                    revision: Some(all.revision.clone()),
                }),
                first: Some(2),
                ..Default::default()
            },
            ConversationRecordPageRequest {
                after: Some(ConversationPositionKey {
                    revision: Some("stale".into()),
                    ..first.clone()
                }),
                first: Some(2),
                ..Default::default()
            },
        ] {
            compare(&request);
        }
    }
}
