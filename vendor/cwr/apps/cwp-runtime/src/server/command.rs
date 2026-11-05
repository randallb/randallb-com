#[expect(
    clippy::too_many_lines,
    reason = "The exhaustive service command dispatch keeps each command's mutation and publication together."
)]
fn execute_command(
    state: &SharedState,
    writer: &SharedWriter,
    command: RuntimeServiceCommand,
) -> CwrResult<()> {
    validate_service_version(&command)?;
    match command {
        RuntimeServiceCommand::ArmConfiguredResident {
            request_id,
            nonce,
            expected_runtime_ref,
            expected_owner_pid,
            ..
        } => retirement::arm(
            state,
            writer,
            request_id,
            nonce,
            expected_runtime_ref,
            expected_owner_pid,
        ),
        RuntimeServiceCommand::RetireConfiguredResident {
            request_id,
            nonce,
            expected_runtime_ref,
            expected_owner_pid,
            ..
        } => retirement::attempt(
            state,
            writer,
            request_id,
            nonce,
            expected_runtime_ref,
            expected_owner_pid,
        ),
        RuntimeServiceCommand::DevGenerationRestart {
            request_id,
            operation,
            generation,
            ..
        } => restart::control(state, writer, request_id, &operation, generation),
        RuntimeServiceCommand::Upstream {
            request_id,
            host_actor_ref,
            command,
            ..
        } => execute_upstream_command(state, writer, request_id, host_actor_ref, command),
        RuntimeServiceCommand::Drain {
            request_id,
            operation,
            ..
        } => drain::control(state, writer, request_id, &operation),
        RuntimeServiceCommand::ExitWhenIdle { request_id, .. } => {
            state.activity.exit_when_idle();
            command_succeeded(writer, request_id, None, Vec::new(), None)
        }

        RuntimeServiceCommand::Snapshot { request_id, .. } => {
            write_runtime_snapshot(state, writer, request_id)
        }
        RuntimeServiceCommand::ReadRootConversations {
            request_id,
            page,
            timeout_ms,
            ..
        } => {
            page.validate()?;
            if !(1..=30_000).contains(&timeout_ms) {
                return invalid("conversation read timeout must be between 1 and 30000 ms");
            }
            let deadline = Instant::now()
                .checked_add(Duration::from_millis(timeout_ms))
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "conversation read deadline overflow",
                    )
                })?;
            let state = Arc::clone(state);
            let writer = Arc::clone(writer);
            thread::Builder::new()
                .name("cwp-runtime-read-roots".into())
                .spawn(move || {
                    let reply_guard = writer::read_reply_guard(&writer, &request_id, deadline);
                    let result = match &reply_guard {
                        Err(error) => invalid(error.to_string()),
                        Ok(_) => loop {
                            if !writer::writer_is_connected(&writer) {
                                break invalid("conversation read operation was cancelled");
                            }
                            if writer::read_was_cancelled(&writer, &request_id) {
                                break invalid("conversation read operation was cancelled");
                            }
                            if Instant::now() >= deadline {
                                break invalid("conversation read operation exceeded its deadline");
                            }
                            match state.state.try_lock() {
                                Ok(state) => break Ok(state.runtime.clone()),
                                Err(std::sync::TryLockError::WouldBlock) => {
                                    thread::sleep(Duration::from_millis(1));
                                }
                                Err(std::sync::TryLockError::Poisoned(_)) => {
                                    break invalid("runtime server state was poisoned");
                                }
                            }
                        },
                    }
                    .and_then(|runtime| {
                        let connection_writer = Arc::clone(&writer);
                        runtime.conversation_host().roots(
                            &page,
                            deadline,
                            || !writer::writer_is_connected(&connection_writer),
                            |cancellation| {
                                writer::register_read_cancellation(
                                    &writer,
                                    &request_id,
                                    cancellation,
                                );
                            },
                        )
                    });
                    let event = match result {
                        Ok(page) => RuntimeServiceEvent::RootConversationsPage { request_id, page },
                        Err(error) => RuntimeServiceEvent::CommandFailed {
                            request_id,
                            error: error.to_string(),
                        },
                    };
                    writer::deliver_read_result(&writer, &event, deadline);
                })?;
            Ok(())
        }
        RuntimeServiceCommand::ReadConversationRecords {
            request_id,
            read,
            timeout_ms,
            ..
        } => {
            read.validate()?;
            if !(1..=30_000).contains(&timeout_ms) {
                return invalid("conversation read timeout must be between 1 and 30000 ms");
            }
            let deadline = Instant::now()
                .checked_add(Duration::from_millis(timeout_ms))
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "conversation read deadline overflow",
                    )
                })?;
            let state = Arc::clone(state);
            let writer = Arc::clone(writer);
            thread::Builder::new()
                .name("cwp-runtime-read-conversation".into())
                .spawn(move || {
                    let reply_guard = writer::read_reply_guard(&writer, &request_id, deadline);
                    let result = match &reply_guard {
                        Err(error) => invalid(error.to_string()),
                        Ok(_) => loop {
                            if !writer::writer_is_connected(&writer)
                                || writer::read_was_cancelled(&writer, &request_id)
                            {
                                break invalid("conversation read operation was cancelled");
                            }
                            if Instant::now() >= deadline {
                                break invalid("conversation read operation exceeded its deadline");
                            }
                            match state.state.try_lock() {
                                Ok(state) => break Ok(state.runtime.clone()),
                                Err(std::sync::TryLockError::WouldBlock) => {
                                    thread::sleep(Duration::from_millis(1));
                                }
                                Err(std::sync::TryLockError::Poisoned(_)) => {
                                    break invalid("runtime server state was poisoned");
                                }
                            }
                        },
                    }
                    .and_then(|runtime| {
                        let connection_writer = Arc::clone(&writer);
                        runtime.conversation_host().records(
                            &read,
                            deadline,
                            || !writer::writer_is_connected(&connection_writer),
                            |cancellation| {
                                writer::register_read_cancellation(
                                    &writer,
                                    &request_id,
                                    cancellation,
                                );
                            },
                        )
                    });
                    let event = match result {
                        Ok(page) => {
                            RuntimeServiceEvent::ConversationRecordsPage { request_id, page }
                        }
                        Err(error) => RuntimeServiceEvent::CommandFailed {
                            request_id,
                            error: error.to_string(),
                        },
                    };
                    writer::deliver_read_result(&writer, &event, deadline);
                })?;
            Ok(())
        }
        RuntimeServiceCommand::ReadVisualization {
            request_id,
            artifact_ref,
            ..
        } => {
            let artifact = lock_state(state)?
                .runtime
                .visualization_artifact(&artifact_ref)?;
            write_event(
                writer,
                &RuntimeServiceEvent::VisualizationRead {
                    request_id,
                    artifact,
                },
            )
        }
        RuntimeServiceCommand::CancelRead {
            request_id,
            target_request_id,
            ..
        } => {
            writer::cancel_read(writer, &target_request_id);
            command_succeeded(writer, request_id, None, Vec::new(), None)
        }
        RuntimeServiceCommand::InspectActor {
            request_id,
            actor_ref,
            ..
        } => inspect_actor(state, writer, request_id, actor_ref.as_deref()),
        RuntimeServiceCommand::ReplyPersonRequest {
            request_id,
            actor_ref,
            conversation_ref,
            invocation_ref,
            text,
            ..
        } => {
            let runtime = {
                let locked = lock_state(state)?;
                if locked.host_actors.get(&upstream_owner(writer)) != Some(&actor_ref) {
                    return invalid("reply requires the connection's selected actor");
                }
                locked.runtime.clone()
            };
            let admission = runtime.reply_person_request_in_upstream_scope(
                &actor_ref,
                &conversation_ref,
                &invocation_ref,
                text,
                Some(&upstream_owner(writer)),
            )?;
            accept_admission(state, writer, request_id, admission, None)
        }
        RuntimeServiceCommand::SubmitText {
            request_id,
            integration_ref,
            author_actor_ref,
            conversation_ref,
            text,
            ..
        } => submit_text_command(
            state,
            writer,
            request_id,
            integration_ref,
            author_actor_ref,
            conversation_ref,
            text,
        ),
        RuntimeServiceCommand::ReplayText {
            request_id,
            integration_ref,
            author_actor_ref,
            conversation_ref,
            text,
            ..
        } => replay_text_command(
            state,
            writer,
            request_id,
            integration_ref,
            author_actor_ref,
            conversation_ref,
            text,
        ),
        command @ (RuntimeServiceCommand::SubmitAction { .. }
        | RuntimeServiceCommand::RegisterGateway { .. }
        | RuntimeServiceCommand::InspectGatewayInvocation { .. }) => {
            execute_gateway_command(state, writer, command)
        }
        RuntimeServiceCommand::RecordHostExchange {
            request_id,
            requesting_actor_ref,
            host_actor_ref,
            conversation_ref,
            request_text,
            response_text,
            ..
        } => {
            let _publication = lock_broadcast(state)?;
            let runtime = lock_state(state)?.runtime.clone();
            let mut exchange = runtime.conversation_host().publish_exchange(
                conversation_ref.as_deref(),
                &requesting_actor_ref,
                &host_actor_ref,
                request_text,
                response_text,
            )?;
            exchange.conversation = runtime
                .conversation_host()
                .conversation(&exchange.conversation.conversation_ref)?
                .map_or_else(
                    || project_conversation(&exchange.conversation),
                    |conversation| project_conversation(&conversation),
                );
            let mut response_exchange = exchange.clone();
            if writer::uses_metadata_bootstrap(writer) {
                response_exchange.conversation.messages.clear();
                response_exchange.conversation.activity.clear();
            }
            write_event(
                writer,
                &RuntimeServiceEvent::CommandSucceeded {
                    request_id,
                    admission: None,
                    stopped_invocation_refs: Vec::new(),
                    intrinsic_response: None,
                    host_exchange: Some(Box::new(response_exchange)),
                },
            )?;
            broadcast_in_publication(
                state,
                RuntimeServiceEvent::ConversationUpsert {
                    conversation: exchange.conversation,
                },
            )
        }
        RuntimeServiceCommand::AdmitIngress {
            host_actor_ref,
            request_id,
            ingress,
            actor_integration_ref,
            attachment,
            ..
        } => ingress::admit(
            state,
            writer,
            request_id,
            host_actor_ref,
            ingress,
            actor_integration_ref,
            attachment,
        ),
        RuntimeServiceCommand::StopConversation {
            request_id,
            conversation_ref,
            requesting_actor_ref,
            invocation_ref,
            event_at_unix_ms,
            ..
        } => {
            let state = Arc::clone(state);
            let writer = Arc::clone(writer);
            thread::Builder::new()
                .name("cwr-host-stop".into())
                .spawn(move || {
                    if let Err(error) = control::stop_conversation(
                        &state,
                        &writer,
                        request_id.clone(),
                        conversation_ref,
                        requesting_actor_ref,
                        invocation_ref,
                        event_at_unix_ms,
                    ) {
                        let _ = write_event(
                            &writer,
                            &RuntimeServiceEvent::CommandFailed {
                                request_id,
                                error: error.to_string(),
                            },
                        );
                    }
                })?;
            Ok(())
        }
        RuntimeServiceCommand::SetConversationLiveStatus {
            request_id,
            conversation_ref,
            subject_actor_ref,
            publisher_session_id,
            sequence,
            target_actor_refs,
            status,
            lease_ms,
            ..
        } => set_conversation_live_status(
            state,
            writer,
            request_id,
            conversation_ref,
            subject_actor_ref,
            publisher_session_id,
            sequence,
            target_actor_refs,
            status,
            lease_ms,
        ),
        RuntimeServiceCommand::ExecuteIntrinsic {
            request_id,
            operation_id,
            command,
            requesting_actor_ref,
            conversation_ref,
            ..
        } => {
            if matches!(
                command,
                cwr::intrinsic_cwr::IntrinsicCwrCommand::ArchiveConversation
                    | cwr::intrinsic_cwr::IntrinsicCwrCommand::RestoreConversation
            ) {
                intrinsic::execute_archive_in_background(
                    state,
                    writer,
                    request_id,
                    operation_id,
                    command,
                    requesting_actor_ref,
                    conversation_ref,
                )
            } else {
                intrinsic::execute(
                    state,
                    writer,
                    request_id,
                    operation_id,
                    &command,
                    &requesting_actor_ref,
                    conversation_ref.as_deref(),
                )
            }
        }
        RuntimeServiceCommand::ReloadGraph {
            request_id,
            requesting_actor_ref,
            ..
        } => reload_graph(state, writer, request_id, requesting_actor_ref),
        RuntimeServiceCommand::RegisterCommunicationAdapter {
            request_id,
            adapter_session_ref,
            integration_ref,
            destinations,
            ..
        } => {
            lock_state(state)?.communication_adapters.insert(
                integration_ref.clone(),
                RegisteredCommunicationAdapter {
                    _session_ref: adapter_session_ref,
                    destinations,
                    writer: Arc::clone(writer),
                },
            );
            command_succeeded(writer, request_id, None, Vec::new(), None)?;
            attachments::replay_for_adapter(state, &integration_ref)?;
            attachments::backfill_pending_defaults(state)
        }
        RuntimeServiceCommand::AttachmentCreated {
            request_id,
            attachment_ref,
            ..
        } => {
            let conversation_ref = attachments::mark_attached(state, &attachment_ref)?;
            command_succeeded(writer, request_id, None, Vec::new(), None)?;
            attachments::request_deliveries(state, &conversation_ref)
        }
        RuntimeServiceCommand::DeliverySucceeded {
            request_id,
            attachment_ref,
            delivery_ref,
            ..
        } => {
            attachments::retain_successful_delivery(state, &attachment_ref, &delivery_ref)?;
            command_succeeded(writer, request_id, None, Vec::new(), None)
        }
        RuntimeServiceCommand::AttachmentFailed {
            request_id,
            attachment_ref,
            error,
            ..
        } => {
            eprintln!("communication attachment {attachment_ref} failed: {error}");
            attachments::retry_attachment_after_failure(state, &attachment_ref)?;
            command_succeeded(writer, request_id, None, Vec::new(), None)
        }
        RuntimeServiceCommand::DeliveryFailed {
            permanent,
            retry_after,
            request_id,
            attachment_ref,
            delivery_ref,
            error,
            ..
        } => {
            let diagnostic = serde_json::from_str::<serde_json::Value>(&error).ok();
            if should_report_delivery_failure(diagnostic.as_ref()) {
                eprintln!("communication delivery {delivery_ref} failed: {error}");
            }
            if permanent {
                attachments::retain_permanent_delivery_failure(
                    state,
                    &attachment_ref,
                    &delivery_ref,
                )?;
            } else {
                let retry_delay = retry_after.map(|retry_after| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    std::time::Duration::from_millis(
                        retry_after.saturating_sub(now).clamp(1_000, 60_000),
                    )
                });
                attachments::retry_delivery_after_failure(
                    state,
                    &attachment_ref,
                    &delivery_ref,
                    retry_delay,
                )?;
            }
            command_succeeded(writer, request_id, None, Vec::new(), None)
        }
    }
}

fn should_report_delivery_failure(diagnostic: Option<&serde_json::Value>) -> bool {
    diagnostic
        .and_then(|value| value.get("report"))
        .and_then(serde_json::Value::as_bool)
        != Some(false)
}

fn validate_service_version(command: &RuntimeServiceCommand) -> CwrResult<()> {
    if command.version() == RUNTIME_SERVICE_VERSION {
        Ok(())
    } else {
        invalid(format!(
            "unsupported runtime service version {}",
            command.version()
        ))
    }
}
