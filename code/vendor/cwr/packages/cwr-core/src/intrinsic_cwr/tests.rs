#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_and_conversational_commands() {
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr--list--coworkers"),
            Some(IntrinsicCwrCommand::ListCoworkers)
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr list coworkers"),
            Some(IntrinsicCwrCommand::ListCoworkers)
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@CWR help"),
            Some(IntrinsicCwrCommand::ShowHelp)
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr--browse--conversation"),
            Some(IntrinsicCwrCommand::BrowseConversation {
                request: default_conversation_browse_request()
            })
        );
    }

    #[test]
    fn browse_request_uses_one_flat_search_contract_with_defaults() {
        let request: ConversationBrowseRequest = serde_json::from_value(serde_json::json!({
            "conversation_ref": "cwp://runtime/conversations/main",
            "operation": {"kind": "search", "query": "deployment"}
        }))
        .unwrap();
        assert_eq!(
            request.conversation_ref.as_deref(),
            Some("cwp://runtime/conversations/main")
        );
        assert!(matches!(
            request.operation,
            ConversationBrowseOperation::Search {
                limit: 20,
                excerpt_bytes: 1024,
                ..
            }
        ));
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr--reload--graph"),
            Some(IntrinsicCwrCommand::ReloadGraph)
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr reload"),
            Some(IntrinsicCwrCommand::ReloadGraph)
        );
        assert_eq!(
            parse_intrinsic_cwr_command(
                "@cwr--debug--invocation cwp://Runtime/Invocations/CaseSensitive"
            ),
            Some(IntrinsicCwrCommand::DebugInvocation {
                invocation_ref: Some("cwp://Runtime/Invocations/CaseSensitive".into()),
            })
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr debug invocation AbC123"),
            Some(IntrinsicCwrCommand::DebugInvocation {
                invocation_ref: Some("AbC123".into()),
            })
        );
        assert_eq!(
            parse_intrinsic_cwr_command(r#"@cwr--debug--invocation {"invocation_ref":"AbC123"}"#),
            Some(IntrinsicCwrCommand::DebugInvocation {
                invocation_ref: Some("AbC123".into()),
            })
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr debug invocation"),
            Some(IntrinsicCwrCommand::DebugInvocation {
                invocation_ref: None,
            })
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr--stop--invocation AbC123"),
            Some(IntrinsicCwrCommand::StopInvocation {
                invocation_ref: Some("AbC123".into()),
            })
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr stop"),
            Some(IntrinsicCwrCommand::StopInvocation {
                invocation_ref: None,
            })
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr cancel cwp://Runtime/Invocations/CaseSensitive"),
            Some(IntrinsicCwrCommand::StopInvocation {
                invocation_ref: Some("cwp://Runtime/Invocations/CaseSensitive".into()),
            })
        );
        assert_eq!(
            parse_intrinsic_cwr_command(
                "@cwr--rename--conversation --title \"Apple local adapter design\""
            ),
            Some(IntrinsicCwrCommand::RenameConversation {
                title: "Apple local adapter design".into(),
            })
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr--rename--conversation --titles Launch"),
            Some(IntrinsicCwrCommand::Unknown(
                "@cwr--rename--conversation --titles Launch".into()
            ))
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr rename conversation A shared title"),
            Some(IntrinsicCwrCommand::RenameConversation {
                title: "A shared title".into(),
            })
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr Rename Conversation Preserve This Case"),
            Some(IntrinsicCwrCommand::RenameConversation {
                title: "Preserve This Case".into(),
            })
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr--archive--conversation"),
            Some(IntrinsicCwrCommand::ArchiveConversation)
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr--restore--conversation"),
            Some(IntrinsicCwrCommand::RestoreConversation)
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr list conversations"),
            Some(IntrinsicCwrCommand::ListConversations {
                page: default_conversation_summary_page(),
                archived: None,
            })
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr read conversation cwp://runtime/conversations/one"),
            Some(IntrinsicCwrCommand::ReadConversation {
                conversation_ref: Some("cwp://runtime/conversations/one".into()),
                page: default_conversation_record_page(),
            })
        );
        assert_eq!(
            parse_intrinsic_cwr_command("@cwr--read--conversation"),
            Some(IntrinsicCwrCommand::ReadConversation {
                conversation_ref: None,
                page: default_conversation_record_page(),
            })
        );
        assert_eq!(parse_intrinsic_cwr_command("@helper hello"), None);
    }

    #[test]
    fn executes_against_one_shared_catalog() {
        let catalog = IntrinsicCwrCatalog {
            coworkers: vec![IntrinsicCwrCatalogCoworker {
                details: Default::default(),
                handle: "@helper".into(),
                name: "Helper".into(),
                actions: vec!["@helper--draft--brief".into()],
            }],
        };
        let IntrinsicCwrResponse::Coworkers { coworkers } =
            response(execute_intrinsic_cwr_command(
                &IntrinsicCwrCommand::ListCoworkers,
                &catalog,
                IntrinsicCwrHostCapabilities::default(),
            ))
        else {
            panic!("expected coworkers")
        };
        assert_eq!(coworkers[0].handle, "@cwr");
        assert_eq!(coworkers[1].handle, "@helper");

        let IntrinsicCwrResponse::Actions { actions, .. } =
            response(execute_intrinsic_cwr_command(
                &IntrinsicCwrCommand::ListActions,
                &catalog,
                IntrinsicCwrHostCapabilities::default(),
            ))
        else {
            panic!("expected actions")
        };
        assert!(actions.contains(&"@cwr--show--help".into()));
        assert!(actions.contains(&"@helper--draft--brief".into()));
    }

    #[test]
    fn renders_unavailable_coworkers_with_the_provider_message() {
        let response = IntrinsicCwrResponse::Coworkers {
            coworkers: vec![IntrinsicCwrCatalogCoworker {
                details: BTreeMap::from([(
                    "bindings".into(),
                    json!([{
                        "available": false,
                        "readiness": {"message": "Install Codex, then retry."}
                    }]),
                )]),
                handle: "@helper".into(),
                name: "Helper".into(),
                actions: Vec::new(),
            }],
        };
        assert_eq!(
            render_intrinsic_cwr_response(&response),
            "Available coworkers:\n@helper — Helper (unavailable: Install Codex, then retry.)"
        );
    }

    #[test]
    fn renders_a_coworker_available_when_any_binding_is_available() {
        let response = IntrinsicCwrResponse::Coworkers {
            coworkers: vec![IntrinsicCwrCatalogCoworker {
                details: BTreeMap::from([(
                    "bindings".into(),
                    json!([
                        {"available": true, "readiness": {"status": "ready"}},
                        {"available": false, "readiness": {
                            "status": "setup_required", "message": "Optional setup missing."
                        }}
                    ]),
                )]),
                handle: "@helper".into(),
                name: "Helper".into(),
                actions: Vec::new(),
            }],
        };
        assert_eq!(
            render_intrinsic_cwr_response(&response),
            "Available coworkers:\n@helper — Helper"
        );
    }

    #[test]
    fn renders_authority_limited_bindings_without_a_provider_claim() {
        let response = IntrinsicCwrResponse::Coworkers {
            coworkers: vec![IntrinsicCwrCatalogCoworker {
                details: BTreeMap::from([(
                    "bindings".into(),
                    json!([
                        {"available": false, "default": true, "readiness": {
                            "status": "ready", "message": "The provider is ready."
                        }},
                        {"available": false, "default": false, "readiness": {
                            "status": "setup_required",
                            "message": "Install an unrelated provider, then retry."
                        }}
                    ]),
                )]),
                handle: "@helper".into(),
                name: "Helper".into(),
                actions: Vec::new(),
            }],
        };
        assert_eq!(
            render_intrinsic_cwr_response(&response),
            "Available coworkers:\n@helper — Helper (unavailable: No authorized route is available.)"
        );
    }

    #[test]
    fn advertises_and_dispatches_reload_only_for_capable_hosts() {
        let catalog = IntrinsicCwrCatalog::default();
        let unavailable = execute_intrinsic_cwr_command(
            &IntrinsicCwrCommand::ReloadGraph,
            &catalog,
            IntrinsicCwrHostCapabilities::default(),
        );
        assert!(matches!(
            unavailable,
            IntrinsicCwrDispatch::Response(IntrinsicCwrResponse::Unavailable { .. })
        ));
        assert!(
            !intrinsic_cwr_actions(IntrinsicCwrHostCapabilities::default())
                .contains(&RELOAD_GRAPH_ACTIVATION.into())
        );

        let capabilities = IntrinsicCwrHostCapabilities {
            reload_graph: true,
            ..Default::default()
        };
        assert_eq!(
            execute_intrinsic_cwr_command(
                &IntrinsicCwrCommand::ReloadGraph,
                &catalog,
                capabilities,
            ),
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ReloadGraph)
        );
        assert!(intrinsic_cwr_actions(capabilities).contains(&RELOAD_GRAPH_ACTIVATION.into()));
    }

    #[test]
    fn advertises_and_dispatches_debug_only_for_capable_hosts() {
        let catalog = IntrinsicCwrCatalog::default();
        let command = IntrinsicCwrCommand::DebugInvocation {
            invocation_ref: Some("abc123".into()),
        };
        assert!(matches!(
            execute_intrinsic_cwr_command(
                &command,
                &catalog,
                IntrinsicCwrHostCapabilities::default(),
            ),
            IntrinsicCwrDispatch::Response(IntrinsicCwrResponse::Unavailable { .. })
        ));
        let capabilities = IntrinsicCwrHostCapabilities {
            debug_invocation: true,
            ..Default::default()
        };
        assert_eq!(
            execute_intrinsic_cwr_command(&command, &catalog, capabilities),
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::DebugInvocation {
                invocation_ref: Some("abc123".into()),
            })
        );
        assert!(intrinsic_cwr_actions(capabilities).contains(&DEBUG_INVOCATION_ACTIVATION.into()));
    }

    #[test]
    fn advertises_and_dispatches_stop_only_for_capable_hosts() {
        let catalog = IntrinsicCwrCatalog::default();
        let command = IntrinsicCwrCommand::StopInvocation {
            invocation_ref: Some("abc123".into()),
        };
        assert!(matches!(
            execute_intrinsic_cwr_command(
                &command,
                &catalog,
                IntrinsicCwrHostCapabilities::default(),
            ),
            IntrinsicCwrDispatch::Response(IntrinsicCwrResponse::Unavailable { .. })
        ));
        let capabilities = IntrinsicCwrHostCapabilities {
            stop_invocation: true,
            ..Default::default()
        };
        assert_eq!(
            execute_intrinsic_cwr_command(&command, &catalog, capabilities),
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::StopInvocation {
                invocation_ref: Some("abc123".into()),
            })
        );
        assert!(intrinsic_cwr_actions(capabilities).contains(&STOP_INVOCATION_ACTIVATION.into()));
    }

    #[test]
    fn advertises_and_dispatches_rename_only_for_capable_hosts() {
        let catalog = IntrinsicCwrCatalog::default();
        let command = IntrinsicCwrCommand::RenameConversation {
            title: "A title".into(),
        };
        assert!(matches!(
            execute_intrinsic_cwr_command(
                &command,
                &catalog,
                IntrinsicCwrHostCapabilities::default(),
            ),
            IntrinsicCwrDispatch::Response(IntrinsicCwrResponse::Unavailable { .. })
        ));
        let capabilities = IntrinsicCwrHostCapabilities {
            rename_conversation: true,
            transfer_conversation: false,
            ..Default::default()
        };
        assert_eq!(
            execute_intrinsic_cwr_command(&command, &catalog, capabilities),
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::RenameConversation {
                title: "A title".into(),
            })
        );
        assert!(
            intrinsic_cwr_actions(capabilities).contains(&RENAME_CONVERSATION_ACTIVATION.into())
        );
    }

    #[test]
    fn advertises_and_dispatches_conversation_reads_only_for_capable_hosts() {
        let catalog = IntrinsicCwrCatalog::default();
        let page = default_conversation_summary_page();
        let command = IntrinsicCwrCommand::ListConversations {
            page: page.clone(),
            archived: Some(false),
        };
        assert!(matches!(
            execute_intrinsic_cwr_command(
                &command,
                &catalog,
                IntrinsicCwrHostCapabilities::default(),
            ),
            IntrinsicCwrDispatch::Response(IntrinsicCwrResponse::Unavailable { .. })
        ));
        let capabilities = IntrinsicCwrHostCapabilities {
            read_conversations: true,
            ..Default::default()
        };
        assert_eq!(
            execute_intrinsic_cwr_command(&command, &catalog, capabilities),
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ListConversations {
                page,
                archived: Some(false),
            })
        );
        let actions = intrinsic_cwr_actions(capabilities);
        assert!(actions.contains(&LIST_CONVERSATIONS_ACTIVATION.into()));
        assert!(actions.contains(&READ_CONVERSATION_ACTIVATION.into()));
        assert!(actions.contains(&BROWSE_CONVERSATION_ACTIVATION.into()));

        let browse = IntrinsicCwrCommand::BrowseConversation {
            request: default_conversation_browse_request(),
        };
        assert!(matches!(
            execute_intrinsic_cwr_command(
                &browse,
                &catalog,
                IntrinsicCwrHostCapabilities::default()
            ),
            IntrinsicCwrDispatch::Response(IntrinsicCwrResponse::Unavailable { .. })
        ));
        assert!(matches!(
            execute_intrinsic_cwr_command(&browse, &catalog, capabilities),
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::BrowseConversation { .. })
        ));
    }

    #[test]
    fn constructs_structured_invocation_stop_responses() {
        assert_eq!(
            intrinsic_cwr_invocation_stop_succeeded(
                "cwp://runtime/invocations/root",
                vec![
                    "cwp://runtime/invocations/root".into(),
                    "cwp://runtime/invocations/child".into(),
                ],
            ),
            IntrinsicCwrResponse::InvocationStopped {
                invocation_ref: "cwp://runtime/invocations/root".into(),
                stopped_invocation_refs: vec![
                    "cwp://runtime/invocations/root".into(),
                    "cwp://runtime/invocations/child".into(),
                ],
            }
        );
        assert_eq!(
            intrinsic_cwr_invocation_stop_too_late("cwp://runtime/invocations/root"),
            IntrinsicCwrResponse::InvocationStopTooLate {
                invocation_ref: "cwp://runtime/invocations/root".into(),
            }
        );
        assert_eq!(
            intrinsic_cwr_invocation_stop_failed(Some("root".into()), "not authorized"),
            IntrinsicCwrResponse::InvocationStopFailed {
                invocation_ref: Some("root".into()),
                error: "not authorized".into(),
            }
        );
    }

    #[test]
    fn reports_sorted_reload_changes() {
        let before = IntrinsicCwrCatalog {
            coworkers: vec![
                coworker("@zebra", "Zebra", &["@zebra--do--thing"]),
                coworker("@changed", "Before", &["@changed--old--action"]),
            ],
        };
        let after = IntrinsicCwrCatalog {
            coworkers: vec![
                coworker("@added", "Added", &["@added--new--action"]),
                coworker("@changed", "After", &["@changed--new--action"]),
            ],
        };
        let response = intrinsic_cwr_reload_succeeded(&before, &after);
        let serialized = serde_json::to_value(&response).unwrap();
        assert!(
            jsonschema::validator_for(&intrinsic_cwr_reload_output_schema())
                .unwrap()
                .is_valid(&serialized)
        );
        assert_eq!(
            serde_json::to_value(IntrinsicCwrReloadInput::default()).unwrap(),
            json!({})
        );
        assert!(
            serde_json::from_value::<IntrinsicCwrReloadInput>(json!({"unexpected": true})).is_err()
        );
        let IntrinsicCwrResponse::Reloaded { changes } = response else {
            panic!("expected reload response")
        };
        assert_eq!(changes.added_coworkers, ["@added"]);
        assert_eq!(changes.removed_coworkers, ["@zebra"]);
        assert_eq!(changes.updated_coworkers, ["@changed"]);
        assert_eq!(
            changes.added_actions,
            ["@added--new--action", "@changed--new--action"]
        );
        assert_eq!(
            changes.removed_actions,
            ["@changed--old--action", "@zebra--do--thing"]
        );
    }

    fn response(dispatch: IntrinsicCwrDispatch) -> IntrinsicCwrResponse {
        let IntrinsicCwrDispatch::Response(response) = dispatch else {
            panic!("expected response")
        };
        response
    }

    fn coworker(handle: &str, name: &str, actions: &[&str]) -> IntrinsicCwrCatalogCoworker {
        IntrinsicCwrCatalogCoworker {
            details: Default::default(),
            handle: handle.into(),
            name: name.into(),
            actions: actions.iter().map(|action| (*action).into()).collect(),
        }
    }
}
