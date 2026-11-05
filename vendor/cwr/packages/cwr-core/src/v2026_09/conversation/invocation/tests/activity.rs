    #[test]
    fn terminal_activities_preserve_exact_status_and_reject_persisted_mismatches() {
        for status in [451, 503, 499] {
            let temporary = tempfile::tempdir().unwrap();
            let path = temporary.path().join(format!("conversation-{status}.json"));
            let mut store = open(&path);
            store.commit(prepared_root()).unwrap();
            store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
            if status == 499 {
                store
                    .stop_invocation(ROOT_INVOCATION_REF, InvocationStopRequest::new(30))
                    .unwrap();
            } else {
                let status = ActionResponseStatus::new(status).unwrap();
                store
                    .finish_invocation(
                        ROOT_INVOCATION_REF,
                        InvocationTerminal {
                            status,
                            summary: "Terminal".into(),
                            result: None,
                            error: Some(ActionResponseDetail::new("terminal")),
                        },
                        30,
                    )
                    .unwrap();
            }

            let exact = store
                .execution_conversation(ROOT_INVOCATION_REF)
                .unwrap()
                .activity
                .iter()
                .find_map(|activity| match activity.event {
                    ConversationActivityEvent::TurnCompleted { status, .. }
                    | ConversationActivityEvent::TurnFailed { status, .. }
                    | ConversationActivityEvent::TurnStopped { status, .. } => Some(status),
                    _ => None,
                })
                .unwrap();
            assert_eq!(exact.as_u16(), status);
            drop(store);

            let mut reopened = open(&path);
            let corrupted = ActionResponseStatus::new(match status {
                451 => 452,
                503 => 504,
                499 => 451,
                _ => unreachable!(),
            })
            .unwrap();
            let mut corrupted_state = reopened.state.clone();
            let execution = corrupted_state
                .conversations
                .iter_mut()
                .find(|conversation| conversation.conversation_ref == EXECUTION_REF)
                .unwrap();
            let terminal = execution
                .activity
                .iter_mut()
                .find(|activity| {
                    matches!(
                        activity.event,
                        ConversationActivityEvent::TurnCompleted { .. }
                            | ConversationActivityEvent::TurnFailed { .. }
                            | ConversationActivityEvent::TurnStopped { .. }
                    )
                })
                .unwrap();
            match &mut terminal.event {
                ConversationActivityEvent::TurnCompleted {
                    status: activity_status,
                    ..
                }
                | ConversationActivityEvent::TurnFailed {
                    status: activity_status,
                    ..
                }
                | ConversationActivityEvent::TurnStopped {
                    status: activity_status,
                    ..
                } => *activity_status = corrupted,
                _ => unreachable!(),
            }
            assert!(validate_invocation_execution_links(&corrupted_state).is_err());
            reopened.persist_state(&corrupted_state).unwrap();
            drop(reopened);
            assert!(ConversationAdmissionStore::open(&path, RUNTIME_ROOT).is_err());
        }

        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("conversation-progress.json");
        let mut store = open(&path);
        store.commit(prepared_root()).unwrap();
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
        store
            .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 150, "Working")
            .unwrap();
        let mut corrupted_state = store.state.clone();
        let execution = corrupted_state
            .conversations
            .iter_mut()
            .find(|conversation| conversation.conversation_ref == EXECUTION_REF)
            .unwrap();
        let progress = execution
            .activity
            .iter_mut()
            .find(|activity| matches!(activity.event, ConversationActivityEvent::Progress { .. }))
            .unwrap();
        let ConversationActivityEvent::Progress { status, .. } = &mut progress.event else {
            unreachable!()
        };
        *status = ActionResponseStatus::new(151).unwrap();
        assert!(validate_invocation_execution_links(&corrupted_state).is_err());
        store.persist_state(&corrupted_state).unwrap();
        drop(store);
        assert!(ConversationAdmissionStore::open(&path, RUNTIME_ROOT).is_err());
    }

    #[test]
    fn progress_activity_matching_handles_duplicate_summaries_with_distinct_statuses() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary
            .path()
            .join("conversation-progress-duplicates.json");
        let mut store = open(&path);
        store.commit(prepared_root()).unwrap();
        store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
        store
            .record_invocation_progress(ROOT_INVOCATION_REF, "Same summary")
            .unwrap();
        let response = store
            .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 150, "Same summary")
            .unwrap();
        assert_eq!(response.status.as_u16(), 150);
        validate_invocation_execution_links(&store.state).unwrap();
    }
