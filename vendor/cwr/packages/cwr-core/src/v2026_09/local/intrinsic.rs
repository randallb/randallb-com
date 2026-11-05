const INTRINSIC_ACTION_NAMES: [(&str, &str, &str); 20] = [
    ("list--schedules", "List schedules", "List schedules owned by the requesting conversation's installed context."),
    ("get--schedule", "Inspect schedule", "Inspect one schedule within the requesting installed context."),
    ("pause--schedule", "Pause schedule", "Pause future admissions for one schedule in the requesting installed context."),
    ("resume--schedule", "Resume schedule", "Resume on a future tick without replaying missed work."),
    ("list--schedule_runs", "Recent schedule runs", "Inspect recent runs and ordinary invocation/conversation links within the requesting installed context."),
    (
        "read--experimental_features",
        "Read experimental features",
        "Read and apply the requesting person’s experimental feature settings.",
    ),
    (
        "set--experimental_features",
        "Set experimental features",
        "Save and apply the requesting person’s experimental feature settings.",
    ),
    (
        "list--coworkers",
        "List coworkers",
        "List coworkers available in this runtime.",
    ),
    (
        "list--actions",
        "List actions",
        "List actions available in this runtime.",
    ),
    ("fork--conversation", "Fork conversation", "Create an independent history through a published timeline item."),
    ("show--help", "Show help", "Show CWR command help."),
    (
        "reload--graph",
        "Reload graph",
        "Reload the owning runtime's workspace graph.",
    ),
    (
        "debug--invocation",
        "Debug invocation",
        "Inspect an exact invocation, or the latest retained failure, including adapter evidence and provider turn scope.",
    ),
    (
        "stop--invocation",
        "Stop invocation",
        "Stop an invocation owned by this runtime.",
    ),
    (
        "list--conversations",
        "List conversations",
        "List root conversations in which the caller participates.",
    ),
    (
        "read--conversation",
        "Read conversation",
        "Read the timeline of a conversation in which the caller participates.",
    ),
    (
        "browse--conversation",
        "Browse conversation",
        "Read recent, related, searched, or excerpted messages in a conversation in which the caller participates. Omit excerpt_bytes for the 1024-byte default; the maximum is 2048 bytes.",
    ),
    (
        "rename--conversation",
        "Rename conversation",
        "Replace the title of the requesting conversation.",
    ),
    (
        "archive--conversation",
        "Archive conversation",
        "Archive the requesting conversation.",
    ),
    (
        "restore--conversation",
        "Restore conversation",
        "Restore the requesting conversation.",
    ),
];

include!("intrinsic/schemas.rs");

fn runtime_graph(mut graph: ResolvedCwrGraph, cwr_toml: &Path) -> CwrResult<ResolvedCwrGraph> {
    let setting = crate::configuration::boolean_setting("desktop.human_reply_inbox")?;
    let disabled: BTreeSet<_> = graph
        .actor_integrations
        .iter()
        .filter(|binding| {
            binding.adapter_ref == crate::adapter::PERSON_ADAPTER_REF
                && !binding
                    .integration_params
                    .get(&setting.workspace_parameter)
                    .and_then(Value::as_bool)
                    .unwrap_or(setting.default)
        })
        .map(|binding| binding.actor_integration_ref.clone())
        .collect();
    graph
        .actor_integrations
        .retain(|binding| !disabled.contains(&binding.actor_integration_ref));
    graph
        .routes
        .retain(|route| !disabled.contains(&route.actor_integration_ref));
    for actor in &mut graph.actors {
        if actor
            .default_actor_integration_ref
            .as_ref()
            .is_some_and(|reference| disabled.contains(reference))
        {
            actor.default_actor_integration_ref = None;
        }
        actor
            .message_binding_preference
            .retain(|reference| !disabled.contains(reference));
    }
    if graph
        .routes
        .iter()
        .any(|route| route.adapter_ref == crate::adapter::PERSON_ADAPTER_REF)
    {
        return invalid(
            "person bindings support conversational respond_to--message only; do not declare named actions",
        );
    }
    // Validate the authored namespace before adding the one runtime-owned actor.
    if graph.actors.iter().any(|actor| {
        actor.actor_ref != INTRINSIC_CWR_ACTOR_REF
            && actor.actor_ref.rsplit('/').next() == Some("cwr")
    }) {
        return invalid("the @cwr actor handle is reserved for the owning runtime");
    }
    let source = cwr_toml.to_path_buf();
    let working_path = cwr_toml
        .parent()
        .ok_or_else(|| invalid_error("CWR.toml has no parent directory"))?
        .to_path_buf();
    graph.resources.push(ResolvedCwrResource {
        source: source.clone(),
        scope_ref: graph.reference.clone(),
        type_ref: "cwp://org.coworkerprotocol/2026-09/schemas/coworker".into(),
        reference: INTRINSIC_CWR_ACTOR_REF.into(),
        visibility: ResolvedCwrVisibility::Export,
        document: json!({
            "type": "cwp://org.coworkerprotocol/2026-09/schemas/coworker",
            "ref": INTRINSIC_CWR_ACTOR_REF,
            "coworker_ref": INTRINSIC_CWR_COWORKER_REF,
            "name": "CWR"
        }),
    });
    graph.actors.push(ResolvedCwrActor {
        default_conversation_target_ref: None,
        native_tools: Vec::new(),
        role_refs: vec![INTRINSIC_CWR_COWORKER_REF.into()],
        message_binding_preference: Vec::new(),
        default_actor_integration_ref: Some(INTRINSIC_CWR_HOST_INTEGRATION_REF.into()),
        actor_ref: INTRINSIC_CWR_ACTOR_REF.into(),
        actor_source: source.clone(),
        actor_name: Some("CWR".into()),
        authority: Vec::new(),
        coworker_ref: INTRINSIC_CWR_COWORKER_REF.into(),
        coworker_source: source.clone(),
        coworker_name: Some("CWR".into()),
        coworker_slug: "cwr".into(),
        category: Some(crate::v2026_09::graph::CoworkerCategory::Software),
        provider_model: None,
    });
    graph.actor_integrations.push(ResolvedCwrActorIntegration {
        actor_ref: INTRINSIC_CWR_ACTOR_REF.into(),
        actor_name: Some("CWR".into()),
        coworker_ref: INTRINSIC_CWR_COWORKER_REF.into(),
        coworker_source: source.clone(),
        working_path: working_path.clone(),
        actor_integration_ref: INTRINSIC_CWR_HOST_INTEGRATION_REF.into(),
        actor_integration_source: source.clone(),
        message_mode: Default::default(),
        integration_ref: INTRINSIC_CWR_INTEGRATION_REF.into(),
        integration_source: source.clone(),
        adapter_ref: INTRINSIC_CWR_ADAPTER_REF.into(),
        adapter_name: "CWR".into(),
        available_authority: Vec::new(),
        missing_required_authority: Vec::new(),
        adapter_params: json!({}),
        integration_params: json!({}),
        actor_integration_params: json!({}),
            input_uses: Vec::new(),
            input_plan: Default::default(),
    });
    for (action_name, title, description) in INTRINSIC_ACTION_NAMES {
        graph.routes.push(ResolvedCwrActionRoute {
            role_ref: INTRINSIC_CWR_COWORKER_REF.into(),
            contract_revision: "1".into(),
            execution_scope: Default::default(),
            response_mode: Default::default(),
            actor_ref: INTRINSIC_CWR_ACTOR_REF.into(),
            coworker_ref: INTRINSIC_CWR_COWORKER_REF.into(),
            coworker_source: source.clone(),
            working_path: working_path.clone(),
            action_name: action_name.into(),
            action_title: title.into(),
            action_description: if matches!(action_name, "list--conversations" | "read--conversation" | "browse--conversation" | "debug--invocation") {
                format!("{} {}", if graph.permissions.allows_all() { description.replace("in which the caller participates", "retained by this runtime") } else { description.to_owned() }, graph.permissions.description())
            } else { description.into() },
            input_schema: intrinsic_action_input_schema(action_name),
            output_schema: intrinsic_action_output_schema(action_name),
            actor_integration_ref: INTRINSIC_CWR_HOST_INTEGRATION_REF.into(),
            actor_integration_source: source.clone(),
            integration_ref: INTRINSIC_CWR_INTEGRATION_REF.into(),
            integration_source: source.clone(),
            adapter_ref: INTRINSIC_CWR_ADAPTER_REF.into(),
            available_authority: Vec::new(),
            missing_required_authority: Vec::new(),
            adapter_params: json!({}),
            integration_params: json!({}),
            actor_integration_params: json!({}),
            input_uses: Vec::new(),
            input_plan: Default::default(),
        });
    }
    retain_native_tool_definitions(&mut graph)?;
    Ok(graph)
}

impl LocalRuntime {
    fn finish_intrinsic_execution(
        &self,
        record: &ActionInvocationRecord,
        invocation: &AdapterInvocation,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
        metrics: &mut crate::metrics::AttemptMetrics,
    ) -> CwrResult<()> {
        let outcome = self.execute_intrinsic_invocation(record, invocation);
        self.lock()?
            .host_local_invocation_owners
            .remove(&record.invocation_ref);
        self.lock()?.active.remove(&record.invocation_ref);
        if self.settle_stopped_execution(&record.invocation_ref)? {
            metrics.outcome = "stopped".into();
            self.finish_attempt_metrics(metrics);
            return Ok(());
        }
        let settlement = match outcome {
            Ok((terminal, mutation)) => {
                metrics.outcome = "completed".into();
                self.publish_terminal_with_mutation(record, terminal, mutation, queue, observer)
            }
            Err(error) => {
                metrics.outcome = "failed".into();
                self.publish_execution_failure(record, error.to_string(), queue, observer)
            }
        };
        let result = self.recover_claim_after_error(&record.invocation_ref, settlement);
        self.finish_attempt_metrics(metrics);
        result
    }

    fn execute_intrinsic_invocation(
        &self,
        record: &ActionInvocationRecord,
        invocation: &AdapterInvocation,
    ) -> CwrResult<(AdapterTerminal, Option<ConversationIntrinsicMutation>)> {
        if matches!(
            invocation.action_name.as_str(),
            "read--experimental_features" | "set--experimental_features"
        ) {
            return self
                .execute_experimental_features(record, invocation)
                .map(|terminal| (terminal, None));
        }
        if matches!(invocation.action_name.as_str(), "list--schedules" | "get--schedule" | "pause--schedule" | "resume--schedule" | "list--schedule_runs") {
            return self.execute_schedule_control(record, invocation).map(|terminal| (terminal, None));
        }
        let command = intrinsic_command_from_invocation(invocation)?;
        if matches!(command, IntrinsicCwrCommand::ReloadGraph)
            && record.parent_invocation_ref.is_some()
        {
            require_reconfiguration_authority(&*self.lock()?, &record.requester_actor_ref)?;
        }
        let catalog = self
            .catalog_as(&record.requester_actor_ref)?
            .intrinsic_cwr_catalog();
        let dispatch = execute_intrinsic_cwr_command(
            &command,
            &catalog,
            IntrinsicCwrHostCapabilities {
                reload_graph: true,
                debug_invocation: true,
                stop_invocation: true,
                read_conversations: true,
                rename_conversation: true,
                archive_conversation: true,
                transfer_conversation: true,
            },
        );
        let mut mutation = None;
        let response = match dispatch {
            IntrinsicCwrDispatch::Response(response) => self
                .conversation_intrinsic_response(record, &command)?
                .unwrap_or(response),
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ReloadGraph) => {
                match self.reload_graph_for_connected_actor_validated(
                    &record.requester_actor_ref,
                    |_, _| Ok(()),
                ) {
                    Ok((before, after)) => intrinsic_cwr_reload_succeeded(
                        &before.intrinsic_cwr_catalog(),
                        &after.intrinsic_cwr_catalog(),
                    ),
                    Err(error) => intrinsic_cwr_reload_failed(error.to_string()),
                }
            }
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::DebugInvocation {
                invocation_ref,
            }) => {
                let result = invocation_ref.as_deref().map_or_else(
                    || self.inspect_latest_failed_invocation_for_conversation(record),
                    |invocation_ref| self.inspect_invocation(invocation_ref),
                );
                match result {
                    Ok(report) => intrinsic_cwr_invocation_debug_succeeded(report),
                    Err(error) => {
                        intrinsic_cwr_invocation_debug_failed(invocation_ref, error.to_string())
                    }
                }
            }
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::StopInvocation {
                invocation_ref,
            }) => self.stop_previous_intrinsic_invocation(record, invocation_ref.as_deref())?,
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ListConversations {
                page,
                archived,
            }) => IntrinsicCwrResponse::ConversationsListed {
                response: ConversationListResponse {
                    page: self.participant_root_conversation_summaries(
                        &record.requester_actor_ref,
                        &page,
                        archived,
                        Instant::now() + Duration::from_secs(5),
                    )?,
                },
            },
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ReadConversation {
                conversation_ref,
                page,
            }) => {
                let conversation_ref = conversation_ref
                    .as_deref()
                    .or(record.conversation_ref.as_deref())
                    .ok_or_else(|| {
                        invalid_error(
                            "provide a conversation ref or run this command in a conversation",
                        )
                    })?;
                IntrinsicCwrResponse::ConversationRead {
                    response: ConversationReadResponse {
                        conversation_ref: conversation_ref.into(),
                        records: ConversationReadRecords::Timeline {
                            page: self.participant_conversation_timeline(
                                conversation_ref,
                                &record.requester_actor_ref,
                                &page,
                                Instant::now() + Duration::from_secs(5),
                            )?,
                        },
                    },
                }
            }
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::BrowseConversation {
                request,
            }) => {
                let conversation_ref = request
                    .conversation_ref
                    .as_deref()
                    .or(record.conversation_ref.as_deref())
                    .ok_or_else(|| {
                        invalid_error("provide a conversation ref or run this command in a conversation")
                    })?;
                IntrinsicCwrResponse::ConversationBrowsed {
                    response: self.participant_conversation_browse_with_context(
                        conversation_ref,
                        &record.requester_actor_ref,
                        &request,
                        Instant::now() + Duration::from_secs(5),
                        BrowseDiagnosticContext {
                            invocation_ref: Some(&record.invocation_ref),
                            ..Default::default()
                        },
                    )?,
                }
            }
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ExportConversation) => self.export_conversation_as(record.conversation_ref.as_deref(), &record.requester_actor_ref)?,
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ImportConversation { path }) => self.import_conversation_as(&record.requester_actor_ref, &path, Some(&record.invocation_ref))?,
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ForkConversation { message_ref }) => self.fork_conversation_at_as(
                record.conversation_ref.as_deref(),
                &record.requester_actor_ref,
                &message_ref,
                &record.invocation_ref,
                record.input.get("timeline_item_ref").and_then(Value::as_str),
            )?,
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::RenameConversation {
                title,
            }) => {
                let (response, prepared) = self.prepare_conversation_rename_as(
                    record.conversation_ref.as_deref(),
                    &record.requester_actor_ref,
                    &title,
                )?;
                mutation = Some(ConversationIntrinsicMutation::Rename(prepared));
                response
            }
            IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::SetConversationArchived {
                archived,
            }) => {
                let (response, prepared) = self.prepare_conversation_archive_as(
                    record.conversation_ref.as_deref(),
                    &record.requester_actor_ref,
                    archived,
                )?;
                mutation = Some(ConversationIntrinsicMutation::Archive(prepared));
                response
            }
        };
        let text = render_intrinsic_cwr_response(&response);
        let is_message = invocation.action_name == RESPOND_TO_MESSAGE_ACTION_NAME;
        let status = if !is_message
            && matches!(
                &response,
                IntrinsicCwrResponse::InvocationStopTooLate { .. }
                    | IntrinsicCwrResponse::InvocationStopFailed { .. }
            ) {
            400
        } else {
            200
        };
        let response = serde_json::to_value(&response)?;
        let (result, error) = if is_message {
            (Some(json!({"text": text})), None)
        } else if status == 200 {
            (Some(response), None)
        } else {
            (
                None,
                Some(AdapterError {
                    code: "intrinsic_command_failed".into(),
                    message: Some(text.clone()),
                    details: Some(response),
                }),
            )
        };
        Ok((
            AdapterTerminal {
                invocation_ref: record.invocation_ref.clone(),
                sequence: 1,
                status,
                summary: text,
                result,
                error,
                continuation: None,
            },
            mutation,
        ))
    }

    fn inspect_latest_failed_invocation_for_conversation(
        &self,
        record: &ActionInvocationRecord,
    ) -> CwrResult<crate::v2026_09::conversation::InvocationDebugReport> {
        let state = self.lock()?;
        let conversation_ref = record
            .conversation_ref
            .as_deref()
            .ok_or_else(|| invalid_error("CWR invocation has no conversation"))?;
        let mut report = state
            .admission
            .inspect_latest_failed_invocation_for_conversation(conversation_ref)?;
        if state.graph.permissions.allows_all() {
            state.admission.include_original_invocation(&mut report)?;
        }
        report.diagnostics_directory = Some(self.diagnostics.directory.display().to_string());
        report.diagnostic_evidence = Some(Box::new(
            self.diagnostics
                .evidence_for_invocation(&report.invocation_ref),
        ));
        Ok(report)
    }


}

fn intrinsic_command_from_invocation(
    invocation: &AdapterInvocation,
) -> CwrResult<IntrinsicCwrCommand> {
    if invocation.action_name == RESPOND_TO_MESSAGE_ACTION_NAME {
        let source_ref = invocation
            .input
            .get("source_message_ref")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_error("CWR message invocation has no source message ref"))?;
        let text = invocation
            .thread
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| {
                messages.iter().find(|message| {
                    message.get("message_ref").and_then(Value::as_str) == Some(source_ref)
                })
            })
            .and_then(|message| message.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| invalid_error("CWR source message is unavailable"))?;
        return parse_intrinsic_cwr_command(&format!("@cwr {text}"))
            .ok_or_else(|| invalid_error("CWR message did not contain a command"));
    }
    let selector = invocation
        .input
        .get("invocation_ref")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let title = invocation
        .input
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let conversation_ref = invocation
        .input
        .get("conversation_ref")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(match invocation.action_name.as_str() {
        "list--coworkers" => IntrinsicCwrCommand::ListCoworkers,
        "list--actions" => IntrinsicCwrCommand::ListActions,
        "show--help" => IntrinsicCwrCommand::ShowHelp,
        "reload--graph" => IntrinsicCwrCommand::ReloadGraph,
        "debug--invocation" => IntrinsicCwrCommand::DebugInvocation {
            invocation_ref: selector,
        },
        "stop--invocation" => IntrinsicCwrCommand::StopInvocation {
            invocation_ref: selector,
        },
        "list--conversations" => IntrinsicCwrCommand::ListConversations {
            page: invocation
                .input
                .get("page")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_else(default_conversation_summary_page),
            archived: invocation.input.get("archived").and_then(Value::as_bool),
        },
        "read--conversation" => IntrinsicCwrCommand::ReadConversation {
            conversation_ref,
            page: invocation
                .input
                .get("page")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_else(default_conversation_record_page),
        },
        "browse--conversation" => IntrinsicCwrCommand::BrowseConversation {
            request: serde_json::from_value(invocation.input.clone())?,
        },
        "rename--conversation" => IntrinsicCwrCommand::RenameConversation {
            title: title.ok_or_else(|| invalid_error("conversation rename has no title"))?,
        },
        "fork--conversation" => IntrinsicCwrCommand::ForkConversation {
            message_ref: invocation.input.get("message_ref").and_then(Value::as_str).ok_or_else(|| invalid_error("fork requires message_ref"))?.into(),
        },
        "archive--conversation" => IntrinsicCwrCommand::ArchiveConversation,
        "restore--conversation" => IntrinsicCwrCommand::RestoreConversation,
        action => IntrinsicCwrCommand::Unknown(action.into()),
    })
}

include!("intrinsic/stop.rs");
