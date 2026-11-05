use super::*;

fn parallel_reply_fixture(
    path: &Path,
    conversational: bool,
    count: usize,
) -> Vec<(String, ConversationMessage)> {
    let mut store = open(path);
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let call = store
        .append_action_call(
            CONVERSATION_REF,
            "cwp://com.example.fixture/actors/worker",
            "cwp://com.example.fixture/actors/child",
            "@child--draft--brief",
            json!({}),
            "Shared source".into(),
        )
        .unwrap();
    let refs = (0..count)
        .map(|i| format!("cwp://com.example.runtime/invocations/parallel-{i}"))
        .collect::<Vec<_>>();
    for (i, reference) in refs.iter().enumerate() {
        let mut request = child_request(reference, &format!("parallel-{i}"));
        let mut adapter = child_adapter_invocation(reference);
        if conversational {
            request.action_name = RESPOND_TO_MESSAGE_ACTION_NAME.into();
            adapter.action_name = RESPOND_TO_MESSAGE_ACTION_NAME.into();
        }
        store
            .start_child_invocation(request, &call.message_ref, adapter)
            .unwrap();
        store.claim_invocation(reference, 25).unwrap();
    }
    // Finish out of admission order, with identical text, author and source.
    refs.into_iter()
        .rev()
        .map(|reference| {
            let PublishInvocationTerminalOutcome::Accepted {
                message: Some(message),
                ..
            } = store
                .publish_child_action_terminal(
                    &reference,
                    reply_terminal(),
                    "Same reply".into(),
                    50,
                )
                .unwrap()
            else {
                panic!("first publication must be accepted")
            };
            (reference, message)
        })
        .collect()
}

fn reply_terminal() -> InvocationTerminal {
    InvocationTerminal {
        status: ActionResponseStatus::COMPLETED,
        summary: "Same reply".into(),
        result: Some(json!({"text":"Same reply"})),
        error: None,
    }
}

#[test]
fn shared_reads_parallel_same_target_completions_keep_exact_replies_after_reopen() {
    for conversational in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("state.sqlite");
        let replies = parallel_reply_fixture(&path, conversational, 2);
        assert_ne!(replies[0].1.message_ref, replies[1].1.message_ref);
        let mut store = open(&path);
        let count = store.conversation(CONVERSATION_REF).unwrap().messages.len();
        let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
        let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
        for (reference, message) in &replies {
            assert_eq!(
                store
                    .invocation_record(reference)
                    .unwrap()
                    .reply_message_ref
                    .as_deref(),
                Some(message.message_ref.as_str())
            );
            let detail = operation
                .conversation_invocation(CONVERSATION_REF, reference)
                .unwrap()
                .unwrap();
            assert_eq!(detail.latest_reply.as_ref(), Some(message));
            let PublishInvocationTerminalOutcome::AlreadyTerminal {
                message: replay, ..
            } = store
                .publish_child_action_terminal(reference, reply_terminal(), "Duplicate".into(), 99)
                .unwrap()
            else {
                panic!("replay must not publish again")
            };
            assert_eq!(replay.as_ref(), Some(message));
        }
        assert_eq!(
            store.conversation(CONVERSATION_REF).unwrap().messages.len(),
            count
        );
    }
}

#[test]
fn shared_reads_legacy_child_replies_require_unambiguous_identity() {
    for count in [1, 2] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("state.sqlite");
        let replies = parallel_reply_fixture(&path, true, count);
        cwpdb_store::test_support::invocation_reads::forget_reply_identities(&path).unwrap();
        let mut store = open(&path);
        let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
        let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
        for (reference, message) in &replies {
            let expected = (count == 1).then_some(message);
            assert_eq!(
                operation
                    .conversation_invocation(CONVERSATION_REF, reference)
                    .unwrap()
                    .unwrap()
                    .latest_reply
                    .as_ref(),
                expected
            );
            let PublishInvocationTerminalOutcome::AlreadyTerminal {
                message: replay, ..
            } = store
                .publish_child_action_terminal(reference, reply_terminal(), "Duplicate".into(), 99)
                .unwrap()
            else {
                panic!("legacy terminal must not publish again")
            };
            assert_eq!(replay.as_ref(), expected);
        }
    }
}

#[test]
fn shared_reads_paged_history_keeps_parallel_reply_origins_after_completion() {
    for conversational in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("state.sqlite");
        let mut replies = parallel_reply_fixture(&path, conversational, 2);
        let mut store = open(&path);
        let PublishInvocationTerminalOutcome::Accepted {
            message: Some(root),
            ..
        } = store
            .publish_root_message_completed(
                ROOT_INVOCATION_REF,
                ActionResponseStatus::COMPLETED,
                json!({"text":"Root done"}),
                "Root done".into(),
                60,
            )
            .unwrap()
        else {
            panic!()
        };
        replies.push((ROOT_INVOCATION_REF.into(), root));
        drop(store);
        let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
        let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
        assert!(operation
            .conversation_current(CONVERSATION_REF)
            .unwrap()
            .active_invocations
            .is_empty());

        let mut rows = Vec::new();
        let mut after = None;
        loop {
            let page = operation
                .conversation_history(
                    CONVERSATION_REF,
                    ConversationRecordWindow {
                        first: Some(1),
                        after: after.as_ref(),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_eq!(page.total_count, 5);
            assert_eq!(page.content.len(), 1);
            after = Some(page.content[0].key.clone());
            rows.extend(page.content);
            if !page.has_next_page {
                break;
            }
        }
        assert_eq!(rows.len(), 5);
        assert!(rows[..2]
            .iter()
            .all(|row| row.origin_invocation_ref.is_none()));
        for (row, (reference, message)) in rows[2..].iter().zip(&replies) {
            assert_eq!(&row.message, message);
            assert_eq!(row.origin_invocation_ref.as_ref(), Some(reference));
            let detail = operation
                .conversation_invocation(
                    CONVERSATION_REF,
                    row.origin_invocation_ref.as_deref().unwrap(),
                )
                .unwrap()
                .unwrap();
            assert_eq!(detail.latest_reply.as_ref(), Some(message));
        }
        let mut backward = Vec::new();
        let mut before = None;
        loop {
            let page = operation
                .conversation_history(
                    CONVERSATION_REF,
                    ConversationRecordWindow {
                        last: Some(1),
                        before: before.as_ref(),
                        ..Default::default()
                    },
                )
                .unwrap();
            before = Some(page.content[0].key.clone());
            backward.extend(page.content);
            if !page.has_previous_page {
                break;
            }
        }
        backward.reverse();
        assert_eq!(backward, rows);
        let legacy = operation
            .conversation_content(
                CONVERSATION_REF,
                ConversationRecordWindow {
                    first: Some(10),
                    ..Default::default()
                },
            )
            .unwrap();
        for (old, new) in legacy.content.iter().zip(&rows) {
            assert_eq!(old.key, new.key);
            assert_eq!(old.message, new.message);
            assert!(serde_json::to_value(old)
                .unwrap()
                .get("origin_invocation_ref")
                .is_none());
        }
    }
}

#[test]
fn shared_reads_history_never_guesses_legacy_or_cross_conversation_origins() {
    for count in [1, 2] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("state.sqlite");
        let replies = parallel_reply_fixture(&path, true, count);
        // Even a unique source/author match is not an explicit history origin.
        cwpdb_store::test_support::invocation_reads::forget_reply_identities(&path).unwrap();
        // An exact reply ref owned by a different conversation must not leak in.
        cwpdb_store::test_support::invocation_reads::cross_conversation_reply(
            &path,
            &replies[0].0,
            &replies[0].1.message_ref,
            "cwp://com.example.runtime/conversations/other",
        )
        .unwrap();
        let mut reader = ConversationReadStore::open(&path, RUNTIME_ROOT).unwrap();
        let operation = reader.begin_operation(Duration::from_secs(5)).unwrap();
        let page = operation
            .conversation_history(
                CONVERSATION_REF,
                ConversationRecordWindow {
                    first: Some(10),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.total_count, count + 2);
        assert!(page
            .content
            .iter()
            .all(|row| row.origin_invocation_ref.is_none()));
    }
}
