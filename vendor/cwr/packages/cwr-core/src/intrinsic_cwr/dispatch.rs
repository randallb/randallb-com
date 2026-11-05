pub fn intrinsic_cwr_actions(capabilities: IntrinsicCwrHostCapabilities) -> Vec<String> {
    let mut actions = BTreeSet::from([
        LIST_COWORKERS_ACTIVATION.to_owned(),
        LIST_ACTIONS_ACTIVATION.to_owned(),
        SHOW_HELP_ACTIVATION.to_owned(),
    ]);
    if capabilities.reload_graph {
        actions.insert(RELOAD_GRAPH_ACTIVATION.to_owned());
    }
    if capabilities.debug_invocation {
        actions.insert(DEBUG_INVOCATION_ACTIVATION.to_owned());
    }
    if capabilities.stop_invocation {
        actions.insert(STOP_INVOCATION_ACTIVATION.to_owned());
    }
    if capabilities.read_conversations {
        actions.insert(LIST_CONVERSATIONS_ACTIVATION.to_owned());
        actions.insert(READ_CONVERSATION_ACTIVATION.to_owned());
        actions.insert(BROWSE_CONVERSATION_ACTIVATION.to_owned());
    }
    if capabilities.rename_conversation {
        actions.insert(RENAME_CONVERSATION_ACTIVATION.to_owned());
    }
    if capabilities.archive_conversation {
        actions.insert(ARCHIVE_CONVERSATION_ACTIVATION.to_owned());
        actions.insert(RESTORE_CONVERSATION_ACTIVATION.to_owned());
    }
    actions.into_iter().collect()
}

pub fn execute_intrinsic_cwr_command(
    command: &IntrinsicCwrCommand,
    catalog: &IntrinsicCwrCatalog,
    capabilities: IntrinsicCwrHostCapabilities,
) -> IntrinsicCwrDispatch {
    let actions = || {
        let mut actions = intrinsic_cwr_actions(capabilities)
            .into_iter()
            .collect::<BTreeSet<_>>();
        for coworker in &catalog.coworkers {
            actions.extend(coworker.actions.iter().cloned());
        }
        actions.into_iter().collect()
    };
    let response = match command {
        IntrinsicCwrCommand::ListCoworkers => {
            let intrinsic = IntrinsicCwrCatalogCoworker {
                details: BTreeMap::new(),
                handle: INTRINSIC_CWR_HANDLE.into(),
                name: "CWR".into(),
                actions: intrinsic_cwr_actions(capabilities),
            };
            let mut coworkers = catalog.coworkers.clone();
            coworkers.sort_unstable_by(|left, right| left.handle.cmp(&right.handle));
            coworkers.insert(0, intrinsic);
            IntrinsicCwrResponse::Coworkers { coworkers }
        }
        IntrinsicCwrCommand::ListActions => IntrinsicCwrResponse::Actions {
            targets: BTreeMap::new(),
            actions: actions(),
            input_schemas: BTreeMap::new(),
            descriptions: BTreeMap::new(),
        },
        IntrinsicCwrCommand::ShowHelp => IntrinsicCwrResponse::Help { actions: actions() },
        IntrinsicCwrCommand::ReloadGraph if capabilities.reload_graph => {
            return IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ReloadGraph);
        }
        IntrinsicCwrCommand::ReloadGraph => IntrinsicCwrResponse::Unavailable {
            activation: RELOAD_GRAPH_ACTIVATION.into(),
            reason: "graph reload is unavailable in this host".into(),
        },
        IntrinsicCwrCommand::DebugInvocation { invocation_ref }
            if capabilities.debug_invocation =>
        {
            return IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::DebugInvocation {
                invocation_ref: invocation_ref.clone(),
            });
        }
        IntrinsicCwrCommand::DebugInvocation { .. } => IntrinsicCwrResponse::Unavailable {
            activation: DEBUG_INVOCATION_ACTIVATION.into(),
            reason: "invocation debugging is unavailable in this host".into(),
        },
        IntrinsicCwrCommand::StopInvocation { invocation_ref } if capabilities.stop_invocation => {
            return IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::StopInvocation {
                invocation_ref: invocation_ref.clone(),
            });
        }
        IntrinsicCwrCommand::StopInvocation { .. } => IntrinsicCwrResponse::Unavailable {
            activation: STOP_INVOCATION_ACTIVATION.into(),
            reason: "invocation stopping is unavailable in this host".into(),
        },
        IntrinsicCwrCommand::ListConversations { page, archived }
            if capabilities.read_conversations =>
        {
            return IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ListConversations {
                page: page.clone(),
                archived: *archived,
            });
        }
        IntrinsicCwrCommand::ReadConversation {
            conversation_ref,
            page,
        } if capabilities.read_conversations => {
            return IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ReadConversation {
                conversation_ref: conversation_ref.clone(),
                page: page.clone(),
            });
        }
        IntrinsicCwrCommand::BrowseConversation { request } if capabilities.read_conversations => {
            return IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::BrowseConversation {
                request: request.clone(),
            });
        }
        IntrinsicCwrCommand::ListConversations { .. }
        | IntrinsicCwrCommand::ReadConversation { .. }
        | IntrinsicCwrCommand::BrowseConversation { .. } => IntrinsicCwrResponse::Unavailable {
            activation: match command {
                IntrinsicCwrCommand::ListConversations { .. } => LIST_CONVERSATIONS_ACTIVATION,
                IntrinsicCwrCommand::BrowseConversation { .. } => BROWSE_CONVERSATION_ACTIVATION,
                _ => READ_CONVERSATION_ACTIVATION,
            }
            .into(),
            reason: "conversation reading is unavailable in this host".into(),
        },
        IntrinsicCwrCommand::RenameConversation { title } if capabilities.rename_conversation => {
            return IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::RenameConversation {
                title: title.clone(),
            });
        }
        IntrinsicCwrCommand::RenameConversation { .. } => IntrinsicCwrResponse::Unavailable {
            activation: RENAME_CONVERSATION_ACTIVATION.into(),
            reason: "conversation renaming is unavailable in this host".into(),
        },
        IntrinsicCwrCommand::ArchiveConversation if capabilities.archive_conversation => {
            return IntrinsicCwrDispatch::HostAction(
                IntrinsicCwrHostAction::SetConversationArchived { archived: true },
            );
        }
        IntrinsicCwrCommand::RestoreConversation if capabilities.archive_conversation => {
            return IntrinsicCwrDispatch::HostAction(
                IntrinsicCwrHostAction::SetConversationArchived { archived: false },
            );
        }
        IntrinsicCwrCommand::ArchiveConversation | IntrinsicCwrCommand::RestoreConversation => {
            IntrinsicCwrResponse::Unavailable {
                activation: if matches!(command, IntrinsicCwrCommand::ArchiveConversation) {
                    ARCHIVE_CONVERSATION_ACTIVATION
                } else {
                    RESTORE_CONVERSATION_ACTIVATION
                }
                .into(),
                reason: "conversation archiving is unavailable in this host".into(),
            }
        }
        IntrinsicCwrCommand::ExportConversation if capabilities.transfer_conversation => {
            return IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ExportConversation);
        }
        IntrinsicCwrCommand::ImportConversation { path } if capabilities.transfer_conversation => {
            return IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ImportConversation {
                path: path.clone(),
            });
        }
        IntrinsicCwrCommand::ForkConversation { message_ref }
            if capabilities.transfer_conversation =>
        {
            return IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ForkConversation {
                message_ref: message_ref.clone(),
            });
        }
        IntrinsicCwrCommand::ExportConversation
        | IntrinsicCwrCommand::ImportConversation { .. }
        | IntrinsicCwrCommand::ForkConversation { .. } => IntrinsicCwrResponse::Unavailable {
            activation: match command {
                IntrinsicCwrCommand::ExportConversation => "export_conversation",
                IntrinsicCwrCommand::ImportConversation { .. } => "import_conversation",
                IntrinsicCwrCommand::ForkConversation { .. } => "fork_conversation",
                _ => unreachable!(),
            }
            .into(),
            reason: "conversation transfer is unavailable in this host".into(),
        },
        IntrinsicCwrCommand::Unknown(command) => IntrinsicCwrResponse::Unknown {
            command: command.clone(),
            actions: actions(),
        },
    };
    IntrinsicCwrDispatch::Response(response)
}
