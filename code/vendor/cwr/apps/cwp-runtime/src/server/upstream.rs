fn upstream_owner(writer: &SharedWriter) -> String {
    format!("connection-{:p}", Arc::as_ptr(writer))
}

fn execute_upstream_command(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    host_actor: String,
    command: cwr::v2026_09::upstream::UpstreamCommand,
) -> CwrResult<()> {
    use cwr::v2026_09::upstream::UpstreamCommand;
    let owner = upstream_owner(writer);
    let runtime = {
        let state = lock_state(state)?;
        if state.host_actors.get(&owner) != Some(&host_actor) {
            return Err(std::io::Error::other(
                "upstream commands require the connection's selected actor",
            )
            .into());
        }
        state.runtime.clone()
    };
    runtime.catalog_as(&host_actor)?;
    match command {
        UpstreamCommand::SubmitDelegatedText {
            provider_session_ref,
            author_actor_ref,
            target_actor_ref,
            source,
            text,
        } => {
            let admission = runtime.submit_delegated_text(
                &owner,
                &host_actor,
                &provider_session_ref,
                &author_actor_ref,
                &target_actor_ref,
                source,
                text,
                &request_id,
            )?;
            accept_admission(state, writer, request_id, admission, None)?;
        }
        UpstreamCommand::ConfigureInstalledSchedules {
            owner_context_ref,
            availability,
            declarations,
        } => {
            configure_installed_schedules(
                &runtime,
                &host_actor,
                &owner_context_ref,
                &availability,
                declarations,
            )?;
            state.graphql_changes.send_replace(());
            command_succeeded(writer, request_id, None, Vec::new(), None)?;
        }
        UpstreamCommand::InviteConversationActor {
            conversation_ref,
            actor_ref,
        } => {
            runtime.invite_coworker_as(&conversation_ref, &host_actor, &actor_ref)?;
            state.graphql_changes.send_replace(());
            command_succeeded(writer, request_id, None, Vec::new(), None)?;
        }
        UpstreamCommand::SuspendConversationContexts => {
            runtime.suspend_conversation_contexts()?;
            state.graphql_changes.send_replace(());
            command_succeeded(writer, request_id, None, Vec::new(), None)?;
        }
        UpstreamCommand::SetConversationContexts { policy } => {
            match policy {
                Some(policy) => runtime.configure_conversation_contexts(Some(policy))?,
                None => runtime.configure_conversation_contexts(None)?,
            }
            state.graphql_changes.send_replace(());
            command_succeeded(writer, request_id, None, Vec::new(), None)?;
        }
        UpstreamCommand::Register { coworkers } => {
            let event_writer = Arc::clone(writer);
            let session = runtime.register_upstream_provider(
                &owner,
                coworkers,
                Arc::new(move |event| {
                    write_event(&event_writer, &RuntimeServiceEvent::Upstream { event })
                }),
                upstream_stop_sender(writer)?,
            )?;
            write_event(
                writer,
                &RuntimeServiceEvent::UpstreamRegistered {
                    request_id,
                    provider_session_ref: session,
                },
            )?;
        }
        UpstreamCommand::RegisterGatewayHandlers { handlers } => {
            let event_writer = Arc::clone(writer);
            let session = runtime.register_gateway_handlers(
                &owner,
                handlers,
                Arc::new(move |event| {
                    write_event(&event_writer, &RuntimeServiceEvent::Upstream { event })
                }),
                upstream_stop_sender(writer)?,
            )?;
            write_event(
                writer,
                &RuntimeServiceEvent::UpstreamRegistered {
                    request_id,
                    provider_session_ref: session,
                },
            )?;
        }
        UpstreamCommand::Enable {
            provider_session_ref,
            actor_ref,
            enabled,
        } => {
            runtime.enable_upstream_coworker(
                &owner,
                &provider_session_ref,
                &actor_ref,
                &host_actor,
                enabled,
            )?;
            command_succeeded(writer, request_id, None, Vec::new(), None)?;
            let catalog = runtime.upstream_catalog_as(&host_actor, Some(&owner))?;
            write_event(writer, &RuntimeServiceEvent::CatalogUpdated { catalog })?;
        }
        UpstreamCommand::Complete {
            invocation_ref,
            terminal,
        } => {
            let terminal = serde_json::from_value(terminal)?;
            let writer = Arc::clone(writer);
            let state = Arc::clone(state);
            // Completion can wait for this same connection's delivery-validation
            // response. Keep the command reader free to receive that response.
            std::thread::Builder::new()
                .name("upstream-completion".into())
                .spawn(move || {
                    let result = runtime.complete_upstream_work(&owner, &invocation_ref, terminal);
                    match result {
                        Ok(()) => {
                            if runtime
                                .terminal_execution_for_invocation(&invocation_ref)
                                .is_ok_and(|terminal| {
                                    terminal.is_some_and(|(_, _, response)| {
                                        response.status.is_stopped()
                                    })
                                })
                            {
                                let _ = control::publish_stop_updates(
                                    &state,
                                    &runtime,
                                    &[invocation_ref],
                                    None,
                                );
                            }
                            state.graphql_changes.send_replace(());
                            let _ = command_succeeded(&writer, request_id, None, Vec::new(), None);
                        }
                        Err(error) => {
                            let _ = write_event(
                                &writer,
                                &RuntimeServiceEvent::CommandFailed {
                                    request_id,
                                    error: error.to_string(),
                                },
                            );
                        }
                    }
                })?;
        }
        UpstreamCommand::ProtectResult { invocation_ref } => {
            runtime.protect_upstream_result(&owner, &invocation_ref)?;
            command_succeeded(writer, request_id, None, Vec::new(), None)?;
        }
        UpstreamCommand::ValidateResult {
            validation_ref,
            allowed,
            authority_guards,
        } => {
            runtime.validate_upstream_result_with_authority(&owner, &validation_ref, allowed, authority_guards)?;
            command_succeeded(writer, request_id, None, Vec::new(), None)?;
        }
        UpstreamCommand::RevokeResult { invocation_ref } => {
            spawn_upstream_revocation(runtime,owner,invocation_ref,Arc::clone(writer),request_id)?;
        }
        UpstreamCommand::AcknowledgeStop { invocation_ref } => {
            runtime.acknowledge_upstream_stop(&owner, &invocation_ref)?;
            let acknowledgement = command_succeeded(writer, request_id, None, Vec::new(), None);
            let publication =
                control::publish_stop_updates(state, &runtime, &[invocation_ref], None);
            state.graphql_changes.send_replace(());
            acknowledgement.and(publication)?;
        }
    }
    Ok(())
}

fn configure_installed_schedules(
    runtime: &cwr::v2026_09::local::LocalRuntime,
    host_actor: &str,
    owner: &str,
    availability: &str,
    declarations: Vec<cwr::v2026_09::upstream::InstalledScheduleActivation>,
) -> CwrResult<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    runtime.configure_installed_schedules_as(host_actor, owner, availability, declarations, now)
}

fn spawn_upstream_revocation(
    runtime:cwr::v2026_09::local::LocalRuntime,owner:String,invocation_ref:String,
    writer:SharedWriter,request_id:String,
)->CwrResult<()> {
            // Stopping a consuming turn may need upstream stop acknowledgements.
            std::thread::Builder::new()
                .name("upstream-revocation".into())
                .spawn(
                    move || match runtime.revoke_upstream_result(&owner, &invocation_ref) {
                        Ok(_) => {
                            let _ = command_succeeded(&writer, request_id, None, Vec::new(), None);
                        }
                        Err(error) => {
                            let _ = write_event(
                                &writer,
                                &RuntimeServiceEvent::CommandFailed {
                                    request_id,
                                    error: error.to_string(),
                                },
                            );
                        }
                    },
                )?;
    Ok(())
}
