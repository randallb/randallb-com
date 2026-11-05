#[expect(
    clippy::too_many_lines,
    reason = "The exhaustive host-to-runtime wire translation keeps each command's validation beside its projection."
)]
fn translate_host_command(
    host_command: HostCommand,
    source_ref: &str,
    ingress_integration_ref: &str,
    bound_actor_ref: Option<&str>,
    output: &SharedHostWriter,
    pending: &SharedPendingRequests,
) -> CwrResult<Option<RuntimeServiceCommand>> {
    let request_id = host_command.request_id().to_owned();
    let include_completed_work_refs = matches!(
        &host_command,
        HostCommand::ReadRootConversations { version: 2 | 3, .. }
    );
    let include_sidebar_metadata = matches!(&host_command, HostCommand::ReadRootConversations { version: 3, .. });
    if !accept_command_check(output, &request_id, host_command.validate())? {
        return Ok(None);
    }
    let (conversation_ref, command) = match host_command {
        HostCommand::Upstream { command, .. } => {
            let actor = bound_actor_ref.ok_or_else(|| io::Error::other("upstream providers require a selected host actor"))?;
            (None, RuntimeServiceCommand::Upstream {
                version: RUNTIME_SERVICE_VERSION, request_id: request_id.clone(), host_actor_ref: actor.into(), command,
            })
        }
        HostCommand::InspectWorkspace { .. } | HostCommand::SelectActor { .. } => {
            write_host_event(
                output,
                &HostEvent::CommandFailed {
                    request_id,
                    error: "the host session is already open; workspace setup commands are no longer accepted"
                        .into(),
                },
            )?;
            return Ok(None);
        }
        HostCommand::ReadRootConversations {
            source_ref: requested_source,
            page,
            timeout_ms,
            ..
        } => {
            if !accept_command_check(
                output,
                &request_id,
                require_source(source_ref, &requested_source),
            )? {
                return Ok(None);
            }
            (
                None,
                RuntimeServiceCommand::ReadRootConversations {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: request_id.clone(),
                    page,
                    timeout_ms,
                },
            )
        }
        HostCommand::ReadConversationRecords {
            source_ref: requested_source,
            read,
            timeout_ms,
            ..
        } => {
            if !accept_command_check(
                output,
                &request_id,
                require_source(source_ref, &requested_source),
            )? {
                return Ok(None);
            }
            (
                read.conversation_ref().map(str::to_owned),
                RuntimeServiceCommand::ReadConversationRecords {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: request_id.clone(),
                    read,
                    timeout_ms,
                },
            )
        }
        HostCommand::ReadVisualization {
            source_ref: requested_source,
            artifact_ref,
            ..
        } => {
            if !accept_command_check(
                output,
                &request_id,
                require_source(source_ref, &requested_source),
            )? {
                return Ok(None);
            }
            (
                None,
                RuntimeServiceCommand::ReadVisualization {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: request_id.clone(),
                    artifact_ref,
                },
            )
        }
        HostCommand::CreateConversation {
            source_ref: requested_source,
            author_actor_ref,
            text,
            ..
        } => {
            if !accept_command_check(
                output,
                &request_id,
                require_source(source_ref, &requested_source),
            )? || !accept_claimed_actor(output, &request_id, bound_actor_ref, &author_actor_ref)?
            {
                return Ok(None);
            }
            (
                None,
                RuntimeServiceCommand::SubmitText {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: request_id.clone(),
                    integration_ref: ingress_integration_ref.into(),
                    author_actor_ref,
                    conversation_ref: None,
                    text,
                },
            )
        }
        HostCommand::ReloadGraph {
            source_ref: requested_source,
            ..
        } => {
            // Workspace reload targets this bound runtime, not a conversation.
            let check = if let Some(requested) = requested_source {
                require_source(source_ref, &requested)
            } else {
                Ok(())
            };
            if !accept_command_check(output, &request_id, check)? {
                return Ok(None);
            }
            let Some(actor) = bound_actor_ref else {
                accept_command_check(output, &request_id, Err(io::Error::other("reload requires a selected actor").into()))?;
                return Ok(None);
            };
            (
                None,
                RuntimeServiceCommand::ReloadGraph {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: request_id.clone(),
                    requesting_actor_ref: actor.to_owned(),
                },
            )
        }
        HostCommand::ReplyPersonRequest { source_ref: requested_source, conversation_ref, invocation_ref, text, .. } => {
            if !accept_command_check(output, &request_id, require_source(source_ref, &requested_source))? { return Ok(None); }
            let actor_ref = bound_actor_ref.ok_or_else(|| io::Error::other("reply requires a selected actor"))?.to_owned();
            (Some(conversation_ref.clone()), RuntimeServiceCommand::ReplyPersonRequest {
                version: RUNTIME_SERVICE_VERSION, request_id: request_id.clone(), actor_ref,
                conversation_ref, invocation_ref, text,
            })
        }
        HostCommand::ContinueConversation {
            source_ref: requested_source,
            conversation_ref,
            author_actor_ref,
            text,
            recovery_only,
            ..
        } => {
            if !accept_command_check(
                output,
                &request_id,
                require_source(source_ref, &requested_source),
            )? || !accept_claimed_actor(output, &request_id, bound_actor_ref, &author_actor_ref)?
            {
                return Ok(None);
            }
            let service_command = if recovery_only {
                RuntimeServiceCommand::ReplayText {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: request_id.clone(),
                    integration_ref: ingress_integration_ref.into(),
                    author_actor_ref,
                    conversation_ref: conversation_ref.clone(),
                    text,
                }
            } else {
                RuntimeServiceCommand::SubmitText {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: request_id.clone(),
                    integration_ref: ingress_integration_ref.into(),
                    author_actor_ref,
                    conversation_ref: Some(conversation_ref.clone()),
                    text,
                }
            };
            (Some(conversation_ref), service_command)
        }
        HostCommand::ExecuteConversationControl {
            source_ref: requested_source,
            conversation_ref,
            requesting_actor_ref,
            control,
            ..
        } => {
            if !accept_command_check(
                output,
                &request_id,
                require_source(source_ref, &requested_source),
            )? || !accept_claimed_actor(
                output,
                &request_id,
                bound_actor_ref,
                &requesting_actor_ref,
            )? {
                return Ok(None);
            }
            let (command, operation_id) = match control {
                cwr::v2026_09::host::ConversationControl::Export => {
                    (cwr::intrinsic_cwr::IntrinsicCwrCommand::ExportConversation, None)
                }
                cwr::v2026_09::host::ConversationControl::Import { path, import_id } => {
                    (cwr::intrinsic_cwr::IntrinsicCwrCommand::ImportConversation { path }, Some(import_id))
                }
                cwr::v2026_09::host::ConversationControl::Fork { message_ref, fork_id } => {
                    (cwr::intrinsic_cwr::IntrinsicCwrCommand::ForkConversation { message_ref }, Some(fork_id))
                }
                cwr::v2026_09::host::ConversationControl::Rename { title } => {
                    (cwr::intrinsic_cwr::IntrinsicCwrCommand::RenameConversation { title }, None)
                }
                cwr::v2026_09::host::ConversationControl::Archive => {
                    (cwr::intrinsic_cwr::IntrinsicCwrCommand::ArchiveConversation, None)
                }
                cwr::v2026_09::host::ConversationControl::Restore => {
                    (cwr::intrinsic_cwr::IntrinsicCwrCommand::RestoreConversation, None)
                }
            };
            (
                conversation_ref.clone(),
                RuntimeServiceCommand::ExecuteIntrinsic {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: request_id.clone(),
                    operation_id,
                    requesting_actor_ref,
                    conversation_ref,
                    command,
                },
            )
        }
        HostCommand::RecordHostExchange {
            source_ref: requested_source,
            requesting_actor_ref,
            host_actor_ref,
            conversation_ref,
            request_text,
            response_text,
            ..
        } => {
            if !accept_command_check(
                output,
                &request_id,
                require_source(source_ref, &requested_source),
            )? || !accept_claimed_actor(
                output,
                &request_id,
                bound_actor_ref,
                &requesting_actor_ref,
            )? {
                return Ok(None);
            }
            (
                conversation_ref.clone(),
                RuntimeServiceCommand::RecordHostExchange {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: request_id.clone(),
                    requesting_actor_ref,
                    host_actor_ref,
                    conversation_ref,
                    request_text,
                    response_text,
                },
            )
        }
        HostCommand::AttachmentCreated { attachment_ref, .. } => {
            if bound_actor_ref.is_none() { return Err(io::Error::other("attachment confirmation requires a selected host actor").into()); }
            (None, RuntimeServiceCommand::AttachmentCreated {
                version: RUNTIME_SERVICE_VERSION, request_id: request_id.clone(), attachment_ref,
            })
        }
        HostCommand::AdmitIngress { ingress, actor_integration_ref, attachment, .. } => {
            if bound_actor_ref.is_none() {
                accept_command_check(output, &request_id, Err(io::Error::other("ingress requires a selected host actor").into()))?;
                return Ok(None);
            }
            if !accept_claimed_actor(output, &request_id, bound_actor_ref, &ingress.author_actor_ref)? {
                return Ok(None);
            }
            (None, RuntimeServiceCommand::AdmitIngress {
                version: RUNTIME_SERVICE_VERSION, request_id: request_id.clone(),
                host_actor_ref: bound_actor_ref.map(str::to_owned),
                ingress, actor_integration_ref, attachment,
            })
        }
        HostCommand::InvokeAction {
            source_ref: requested_source,
            author_actor_ref,
            conversation_ref,
            activation,
            input,
            ..
        } => {
            // A source-free action belongs to this selected host session.
            let source_check = if let Some(requested) = requested_source {
                require_source(source_ref, &requested)
            } else if bound_actor_ref.is_some() {
                Ok(())
            } else {
                Err(io::Error::other("workspace actions require a selected host actor").into())
            };
            if !accept_command_check(output, &request_id, source_check)? || !accept_claimed_actor(output, &request_id, bound_actor_ref, &author_actor_ref)?
            {
                return Ok(None);
            }
            (
                conversation_ref.clone(),
                RuntimeServiceCommand::SubmitAction {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: request_id.clone(),
                    integration_ref: ingress_integration_ref.into(),
                    author_actor_ref,
                    conversation_ref,
                    activation,
                    input,
                },
            )
        }
        HostCommand::StopConversation {
            source_ref: requested_source,
            conversation_ref,
            requesting_actor_ref,
            invocation_ref,
            ..
        } => {
            if !accept_command_check(
                output,
                &request_id,
                require_source(source_ref, &requested_source),
            )? || !accept_claimed_actor(
                output,
                &request_id,
                bound_actor_ref,
                &requesting_actor_ref,
            )? {
                return Ok(None);
            }
            (
                Some(conversation_ref.clone()),
                RuntimeServiceCommand::StopConversation {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: request_id.clone(),
                    conversation_ref,
                    requesting_actor_ref,
                    invocation_ref,
                    event_at_unix_ms: None,
                },
            )
        }
        HostCommand::SetConversationLiveStatus {
            source_ref: requested_source,
            conversation_ref,
            subject_actor_ref,
            publisher_session_id,
            sequence,
            target_actor_refs,
            status,
            lease_ms,
            ..
        } => {
            if !accept_command_check(
                output,
                &request_id,
                require_source(source_ref, &requested_source),
            )? || !accept_claimed_actor(
                output,
                &request_id,
                bound_actor_ref,
                &subject_actor_ref,
            )? {
                return Ok(None);
            }
            (
                Some(conversation_ref.clone()),
                RuntimeServiceCommand::SetConversationLiveStatus {
                    version: RUNTIME_SERVICE_VERSION,
                    request_id: request_id.clone(),
                    conversation_ref,
                    subject_actor_ref,
                    publisher_session_id,
                    sequence,
                    target_actor_refs,
                    status,
                    lease_ms,
                },
            )
        }
    };
    if !accept_command_check(
        output,
        &request_id,
        pending::reserve_request(
            pending,
            &request_id,
            conversation_ref,
            include_completed_work_refs,
        ),
    )? {
        return Ok(None);
    }
    if include_sidebar_metadata { pending::mark_sidebar_metadata(pending, &request_id)?; }
    if matches!(
        command,
        RuntimeServiceCommand::ReadRootConversations { .. }
            | RuntimeServiceCommand::ReadConversationRecords { .. }
    ) {
        pending::mark_cancel_on_disconnect(pending, &request_id)?;
    }
    Ok(Some(command))
}
