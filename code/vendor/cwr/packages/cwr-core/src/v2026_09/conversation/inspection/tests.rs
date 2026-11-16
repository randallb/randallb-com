#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::v2026_09::conversation::{
        ConversationActivity, ConversationRecipientVisibility, ConversationRuntimeState,
        RetainedConversation,
    };
    use crate::v2026_09::invocation::ActionResponseStatus;

    fn store() -> ConversationAdmissionStore {
        let runtime_root = "cwp://example.runtime/slack-test";
        let invocation_ref = format!("{runtime_root}/invocations/abc123");
        let execution_ref = format!("{runtime_root}/conversations/execution");
        let diagnostic = json!({
            "attempt": 2,
            "event": "provider_request_failed",
            "operation": "universal-search",
            "provider": "scileads",
            "provider_error": "secret lead ada@example.test",
            "request_id": "request-123",
            "retryable": true,
            "status": 429
        });
        let temporary = tempfile::tempdir().unwrap();
        let mut store = ConversationAdmissionStore::open(
            temporary.path().join("inspection.sqlite3"),
            runtime_root,
        )
        .unwrap();
        store.state = ConversationRuntimeState {
            cold_conversation_refs: Default::default(),
            files: Default::default(),
            version: super::super::STATE_VERSION.into(),
            runtime_root_ref: runtime_root.into(),
            visualization_artifacts: Default::default(),
            invocation_ledger: Default::default(),
            timeline_store: Default::default(),
            streaming_messages: Default::default(),
            streaming_runs: Default::default(),
            tool_attempts: Default::default(),
            tool_operations: Default::default(),
            invocation_execution_conversation_refs: Default::default(),
            upstream_participant_versions: Default::default(),
        protected_result_children: Default::default(),
            admissions: vec![ConversationAdmissionRecord {
                integration_ref: "~@/integrations/slack".into(),
                provider_event_ref: "event-1".into(),
                ingress_fingerprint: None,
                source_input_fingerprint: None,
                conversation_ref: format!("{runtime_root}/conversations/thread"),
                source_message_ref: format!("{runtime_root}/messages/source"),
                execution_conversation_ref: Some(execution_ref.clone()),
                target_actor_ref: None,
                actor_integration_ref: None,
                adapter_ref: None,
                invocation_ref: Some(invocation_ref.clone()),
                state: ConversationAdmissionState::Failed,
                reply_recipient_visibility: ConversationRecipientVisibility::Implicit,
                reply_message_ref: None,
            }]
            .into(),
            conversations: vec![RetainedConversation {
                owner_context_ref: None,
                conversation_ref: execution_ref,
                title: None,
                archived: false,
                sidebar_eligible: None,
                parent_conversation_ref: None,
                participant_actor_refs: Vec::new(),
                messages: Vec::new(),
                timeline_connection_ref: None,
                activity: vec![
                    ConversationActivity {
                        activity_ref: "activity-1".into(),
                        sequence: 1,
                        occurred_at_unix_ms: 1_787_858_714_000,
                        event: ConversationActivityEvent::TurnStarted {
                            invocation_ref,
                            source_message_ref: "secret source".into(),
                            invocation: Some(json!({"thread": "SECRET SLACK THREAD"})),
                        },
                    },
                    ConversationActivity {
                        activity_ref: "activity-2".into(),
                        sequence: 2,
                        occurred_at_unix_ms: 1_787_858_714_000,
                        event: ConversationActivityEvent::ProviderEvent {
                            provider: "codex".into(),
                            method: "item/completed".into(),
                            data: json!({
                                "item": {
                                    "type": "commandExecution",
                                    "aggregatedOutput": format!(
                                        "{diagnostic}\nRAW COMMAND OUTPUT MUST NOT APPEAR"
                                    )
                                }
                            }),
                        },
                    },
                    ConversationActivity {
                        activity_ref: "activity-3".into(),
                        sequence: 3,
                        occurred_at_unix_ms: 0,
                        event: ConversationActivityEvent::ProviderEvent {
                            provider: "codex".into(),
                            method: "item/started".into(),
                            data: json!({
                                "item": {
                                    "namespace": "unrelated",
                                    "tool": "return_result",
                                    "arguments": {"error": {
                                        "code": "MUST_NOT_APPEAR"
                                    }}
                                }
                            }),
                        },
                    },
                    ConversationActivity {
                        activity_ref: "activity-4".into(),
                        sequence: 4,
                        occurred_at_unix_ms: 0,
                        event: ConversationActivityEvent::ProviderEvent {
                            provider: "codex".into(),
                            method: "item/started".into(),
                            data: json!({
                                "item": {
                                    "namespace": "cwr",
                                    "tool": "return_result",
                                    "arguments": {"error": {
                                        "code": "SCILEADS_RATE_LIMITED"
                                    }}
                                }
                            }),
                        },
                    },
                    ConversationActivity {
                        activity_ref: "activity-5".into(),
                        sequence: 5,
                        occurred_at_unix_ms: 0,
                        event: ConversationActivityEvent::TurnFailed {
                            status: ActionResponseStatus::BLOCKED,
                            error: "adapter returned status 422: private summary".into(),
                        },
                    },
                ],
            }]
            .into(),
        };
        store
    }

    #[test]
    fn inspects_only_allowlisted_diagnostics() {
        let report = store().inspect_invocation("abc123").unwrap();
        assert_eq!(report.provider_requests.len(), 1);
        assert_eq!(report.provider_requests[0].status, Some(429));
        assert_eq!(
            report.terminal,
            Some(InvocationDebugTerminal {
                kind: "turn_failed".into(),
                status: Some(422),
                error_codes: vec!["SCILEADS_RATE_LIMITED".into()],
            })
        );
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(!serialized.contains("SECRET SLACK THREAD"));
        assert!(!serialized.contains("RAW COMMAND OUTPUT MUST NOT APPEAR"));
        assert!(!serialized.contains("private summary"));
        assert!(!serialized.contains("secret lead"));
        assert!(!serialized.contains("MUST_NOT_APPEAR"));
    }

    #[test]
    fn requires_exact_short_ids_and_supports_thread_defaults() {
        let store = store();
        assert!(store.inspect_invocation("abc").is_err());
        let report = store
            .inspect_latest_invocation_for_conversation(
                "cwp://example.runtime/slack-test/conversations/thread",
            )
            .unwrap();
        assert!(report.invocation_ref.ends_with("/invocations/abc123"));
    }

    #[test]
    fn thread_failure_default_skips_newer_non_failures() {
        let mut store = store();
        let conversation_ref = "cwp://example.runtime/slack-test/conversations/thread".to_owned();
        let mut completed = store.state.admissions[0].clone();
        completed.provider_event_ref = "event-2".into();
        completed.invocation_ref =
            Some("cwp://example.runtime/slack-test/invocations/newer-completed".into());
        completed.state = ConversationAdmissionState::Completed;
        store.state.admissions.push(completed);

        let latest = store
            .inspect_latest_invocation_for_conversation(&conversation_ref)
            .unwrap();
        assert!(latest.invocation_ref.ends_with("newer-completed"));

        let failed = store
            .inspect_latest_failed_invocation_for_conversation(&conversation_ref)
            .unwrap();
        assert!(failed.invocation_ref.ends_with("abc123"));
    }

    #[test]
    fn reports_completed_status_directly() {
        let mut store = store();
        let conversation = store.state.conversations.first_mut().unwrap();
        conversation.activity.pop();
        conversation.activity.push(ConversationActivity {
            activity_ref: "activity-complete".into(),
            sequence: 6,
            occurred_at_unix_ms: 0,
            event: ConversationActivityEvent::TurnCompleted {
                status: ActionResponseStatus::new(204).unwrap(),
                summary: "private summary".into(),
                result: Some(json!({"secret": "must not appear"})),
            },
        });
        let report = store.inspect_invocation("abc123").unwrap();
        assert_eq!(
            report.terminal.as_ref().and_then(|value| value.status),
            Some(204)
        );
        let serialized = serde_json::to_string(&report).unwrap();
        assert!(!serialized.contains("private summary"));
        assert!(!serialized.contains("must not appear"));
    }

    #[test]
    fn reports_stopped_terminal_without_exposing_the_reason() {
        let mut store = store();
        store.state.admissions[0].state = ConversationAdmissionState::Stopped;
        let conversation = store.state.conversations.first_mut().unwrap();
        conversation.activity.pop();
        conversation.activity.push(ConversationActivity {
            activity_ref: "activity-stopped".into(),
            sequence: 6,
            occurred_at_unix_ms: 0,
            event: ConversationActivityEvent::TurnStopped {
                status: ActionResponseStatus::STOPPED,
                reason: Some("private stop reason".into()),
            },
        });

        let report = store.inspect_invocation("abc123").unwrap();
        assert_eq!(
            report.terminal,
            Some(InvocationDebugTerminal {
                kind: "turn_stopped".into(),
                status: Some(499),
                error_codes: vec!["SCILEADS_RATE_LIMITED".into()],
            })
        );
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("private stop reason"));
    }

    #[test]
    fn inspects_a_retained_child_invocation_without_a_root_admission() {
        let mut store = store();
        let runtime_root = "cwp://example.runtime/slack-test";
        let child_ref = format!("{runtime_root}/invocations/child123");
        let parent_ref = format!("{runtime_root}/invocations/abc123");
        let execution_ref = format!("{runtime_root}/conversations/child-execution");
        store.state.invocation_ledger = serde_json::from_value(json!({
            "invocations": {
                child_ref.clone(): {
                    "invocation_ref": child_ref,
                    "origin": {
                        "kind": "host_call",
                        "caller_invocation_ref": parent_ref,
                        "call_id": "start-child"
                    },
                    "requester_actor_ref": "~@/actors/parent",
                    "target_actor_ref": "~@/actors/child",
                    "conversation_ref": format!("{runtime_root}/conversations/thread"),
                    "actor_integration_ref": "~@/actor_integrations/child",
                    "action_name": "do--work",
                    "input": {},
                    "parent_invocation_ref": parent_ref,
                    "state": "completed",
                    "responses": [{
                        "invocation_ref": child_ref,
                        "sequence": 1,
                        "status": 200,
                        "summary": "Completed child work.",
                        "result": {"ok": true}
                    }],
                    "admitted_at_unix_ms": 1,
                    "started_at_unix_ms": 2,
                    "finished_at_unix_ms": 3,
                    "attempt_count": 1,
                    "attempt_active": false
                }
            }
        }))
        .unwrap();
        store
            .state
            .invocation_execution_conversation_refs
            .insert(child_ref.clone(), execution_ref.clone());
        store.state.conversations.push(RetainedConversation {
            owner_context_ref: None,
            conversation_ref: execution_ref.clone(),
            title: None,
            archived: false,
            sidebar_eligible: None,
            parent_conversation_ref: None,
            participant_actor_refs: Vec::new(),
            messages: Vec::new(),
            timeline_connection_ref: None,
            activity: vec![ConversationActivity {
                activity_ref: "child-terminal".into(),
                sequence: 1,
                occurred_at_unix_ms: 3,
                event: ConversationActivityEvent::TurnCompleted {
                    status: ActionResponseStatus::COMPLETED,
                    summary: "private child summary".into(),
                    result: Some(json!({"secret": "must not appear"})),
                },
            }],
        });

        let report = store.inspect_invocation("child123").unwrap();

        assert_eq!(report.invocation_ref, child_ref);
        assert_eq!(report.execution_conversation_ref, execution_ref);
        let identity = report.identity.unwrap();
        assert_eq!(identity.target_actor_ref, "~@/actors/child");
        assert_eq!(identity.action_name.as_deref(), Some("do--work"));
        assert_eq!(
            report.admission_state,
            ConversationAdmissionState::Completed
        );
        assert_eq!(
            report.terminal,
            Some(InvocationDebugTerminal {
                kind: "turn_completed".into(),
                status: Some(200),
                error_codes: Vec::new(),
            })
        );
    }

    #[test]
    fn canonical_terminal_code_replaces_earlier_provider_attempt_codes() {
        let mut store = store();
        let invocation_ref = "cwp://example.runtime/slack-test/invocations/abc123".to_owned();
        store.state.admissions[0].adapter_ref = Some("~@/adapters/codex".into());
        install_root_terminal(
            &mut store,
            "blocked",
            json!({
                "invocation_ref": invocation_ref,
                "sequence": 1,
                "status": 422,
                "summary": "Approval required.",
                "error": {"code": "approval_required"}
            }),
        );

        let report = store.inspect_invocation("abc123").unwrap();

        assert_eq!(
            report.terminal,
            Some(InvocationDebugTerminal {
                kind: "turn_failed".into(),
                status: Some(422),
                error_codes: vec!["approval_required".into()],
            })
        );
        let identity = report.identity.unwrap();
        assert_eq!(identity.target_actor_ref, "~@/actors/coworker");
        assert_eq!(identity.action_name.as_deref(), Some("respond--message"));
        assert_eq!(identity.adapter_ref.as_deref(), Some("~@/adapters/codex"));
    }

    #[test]
    fn canonical_ledger_state_replaces_a_stale_root_admission_state() {
        let mut store = store();
        let invocation_ref = "cwp://example.runtime/slack-test/invocations/abc123".to_owned();
        store.state.admissions[0].state = ConversationAdmissionState::Admitted;
        install_root_terminal(
            &mut store,
            "blocked",
            json!({
                "invocation_ref": invocation_ref,
                "sequence": 1,
                "status": 422,
                "summary": "Approval required.",
                "error": {"code": "approval_required"}
            }),
        );

        let report = store.inspect_invocation("abc123").unwrap();

        assert_eq!(report.admission_state, ConversationAdmissionState::Failed);
        assert_eq!(
            report.terminal,
            Some(InvocationDebugTerminal {
                kind: "turn_failed".into(),
                status: Some(422),
                error_codes: vec!["approval_required".into()],
            })
        );

        let latest = store
            .inspect_latest_invocation_for_conversation(
                "cwp://example.runtime/slack-test/conversations/thread",
            )
            .unwrap();
        assert_eq!(latest.admission_state, ConversationAdmissionState::Failed);
        assert_eq!(latest.terminal, report.terminal);
    }

    #[test]
    fn canonical_stopped_cause_replaces_earlier_provider_attempt_codes() {
        let mut store = store();
        let invocation_ref = "cwp://example.runtime/slack-test/invocations/abc123".to_owned();
        store.state.admissions[0].state = ConversationAdmissionState::Stopped;
        let conversation = store.state.conversations.first_mut().unwrap();
        conversation.activity.pop();
        conversation.activity.push(ConversationActivity {
            activity_ref: "activity-stopped".into(),
            sequence: 6,
            occurred_at_unix_ms: 0,
            event: ConversationActivityEvent::TurnStopped {
                status: ActionResponseStatus::STOPPED,
                reason: Some("private stop reason".into()),
            },
        });
        install_root_terminal(
            &mut store,
            "stopped",
            json!({
                "invocation_ref": invocation_ref,
                "sequence": 1,
                "status": 499,
                "summary": "Invocation stopped.",
                "cause": {"code": "stop_requested"}
            }),
        );

        let report = store.inspect_invocation("abc123").unwrap();

        assert_eq!(
            report.terminal,
            Some(InvocationDebugTerminal {
                kind: "turn_stopped".into(),
                status: Some(499),
                error_codes: vec!["stop_requested".into()],
            })
        );
        assert_eq!(
            report.lifecycle,
            Some(Box::new(InvocationDebugLifecycle {
                admitted_at_unix_ms: 1,
                started_at_unix_ms: Some(2),
                stop_requested_at_unix_ms: Some(3),
                finished_at_unix_ms: Some(3),
                total_runtime_ms: Some(1),
                stop_to_finish_latency_ms: Some(0),
            }))
        );
    }

    #[test]
    fn distinguishes_delayed_stop_settlement_from_an_immediate_stop() {
        let mut store = store();
        let invocation_ref = "cwp://example.runtime/slack-test/invocations/abc123".to_owned();
        install_root_terminal_with_times(
            &mut store,
            "stopped",
            json!({
                "invocation_ref": invocation_ref,
                "sequence": 1,
                "status": 499,
                "summary": "Invocation stopped.",
                "cause": {"code": "stop_requested"}
            }),
            10,
            35,
        );

        let lifecycle = store
            .inspect_invocation("abc123")
            .unwrap()
            .lifecycle
            .unwrap();

        assert_eq!(lifecycle.total_runtime_ms, Some(33));
        assert_eq!(lifecycle.stop_to_finish_latency_ms, Some(25));
    }

    fn install_root_terminal(store: &mut ConversationAdmissionStore, state: &str, response: Value) {
        install_root_terminal_with_times(store, state, response, 3, 3);
    }

    fn install_root_terminal_with_times(
        store: &mut ConversationAdmissionStore,
        state: &str,
        response: Value,
        stop_requested_at_unix_ms: u64,
        finished_at_unix_ms: u64,
    ) {
        let invocation_ref = "cwp://example.runtime/slack-test/invocations/abc123".to_owned();
        store.state.invocation_ledger = serde_json::from_value(json!({
            "invocations": {
                invocation_ref.clone(): {
                    "invocation_ref": invocation_ref,
                    "origin": {
                        "kind": "ingress",
                        "integration_ref": "~@/integrations/slack",
                        "provider_event_ref": "event-1"
                    },
                    "requester_actor_ref": "~@/actors/viewer",
                    "target_actor_ref": "~@/actors/coworker",
                    "conversation_ref": "cwp://example.runtime/slack-test/conversations/thread",
                    "actor_integration_ref": "~@/actor_integrations/coworker",
                    "action_name": "respond--message",
                    "input": {},
                    "state": state,
                    "responses": [response],
                    "admitted_at_unix_ms": 1,
                    "started_at_unix_ms": 2,
                    "stop_requested_at_unix_ms": (state == "stopped").then_some(stop_requested_at_unix_ms),
                    "finished_at_unix_ms": finished_at_unix_ms,
                    "attempt_count": 1,
                    "attempt_active": false
                }
            }
        }))
        .unwrap();
    }

    #[test]
    fn rejects_malformed_or_oversized_selectors_without_echoing_them() {
        let store = store();
        let malformed = "cwp://example.runtime/invocations/abc\n`spoofed`";
        let error = store.inspect_invocation(malformed).unwrap_err().to_string();
        assert_eq!(error, "invalid invocation ref");
        assert!(!error.contains("spoofed"));

        let oversized = "x".repeat(MAX_INVOCATION_SELECTOR_BYTES + 1);
        let error = store
            .inspect_invocation(&oversized)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "invocation ref or ID is too long");
        assert!(!error.contains(&oversized));
    }
}
