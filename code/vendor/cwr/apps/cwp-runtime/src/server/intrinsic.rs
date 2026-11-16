use cwr::{
    intrinsic_cwr::{
        intrinsic_cwr_invocation_debug_failed, intrinsic_cwr_invocation_debug_succeeded,
        intrinsic_cwr_reload_failed, intrinsic_cwr_reload_succeeded, IntrinsicCwrCommand,
        IntrinsicCwrDispatch, IntrinsicCwrHostAction, IntrinsicCwrHostCapabilities,
    },
    v2026_09::local::LocalIntrinsicCwrHost,
};

use super::*;

pub(super) fn execute(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    operation_id: Option<String>,
    command: &IntrinsicCwrCommand,
    requesting_actor_ref: &str,
    conversation_ref: Option<&str>,
) -> CwrResult<()> {
    execute_reserved(
        state,
        writer,
        (request_id, operation_id),
        command,
        requesting_actor_ref,
        conversation_ref,
        None,
    )
}

fn execute_reserved(
    state: &SharedState,
    writer: &SharedWriter,
    request: (String, Option<String>),
    command: &IntrinsicCwrCommand,
    requesting_actor_ref: &str,
    conversation_ref: Option<&str>,
    reservation: Option<Arc<cwr::v2026_09::local::LocalArchiveReservation>>,
) -> CwrResult<()> {
    let (request_id, operation_id) = request;
    let runtime = lock_state(state)?.runtime.clone();
    let mut archived_stops = Vec::new();
    let response = if matches!(
        command,
        IntrinsicCwrCommand::ArchiveConversation | IntrinsicCwrCommand::RestoreConversation
    ) {
        runtime.catalog_as(requesting_actor_ref)?;
        let archived = matches!(command, IntrinsicCwrCommand::ArchiveConversation);
        let module = runtime.bundled_conversation_module_as(
            "cwp://!cwr/actor_integrations/host",
            requesting_actor_ref,
        )?;
        let (response, stopped) =
            module.set_archived_with_stops(conversation_ref, archived, reservation)?;
        archived_stops = stopped;
        response
    } else {
        execute_intrinsic(
            &runtime,
            command,
            requesting_actor_ref,
            conversation_ref,
            &request_id,
            operation_id.as_deref(),
        )?
    };
    let reloaded = matches!(&response, IntrinsicCwrResponse::Reloaded { .. });
    let stopped = match &response {
        IntrinsicCwrResponse::InvocationStopped {
            stopped_invocation_refs,
            ..
        } => Some(stopped_invocation_refs.clone()),
        _ if !archived_stops.is_empty() => Some(archived_stops),
        _ => None,
    };
    let changed_conversation_ref = match &response {
        IntrinsicCwrResponse::ConversationImported { conversation_ref } => {
            Some(conversation_ref.clone())
        }
        IntrinsicCwrResponse::ConversationForked {
            conversation_ref, ..
        } => Some(conversation_ref.clone()),
        IntrinsicCwrResponse::ConversationRenamed {
            conversation_ref, ..
        } => Some(conversation_ref.clone()),
        IntrinsicCwrResponse::ConversationArchived {
            conversation_ref, ..
        } => Some(conversation_ref.clone()),
        _ => None,
    };
    let catalog_update = if reloaded {
        let _publication = lock_broadcast(state)?;
        broadcast_in_publication(
            state,
            RuntimeServiceEvent::CatalogUpdated {
                catalog: runtime.catalog()?,
            },
        )
    } else {
        Ok(())
    };
    let acknowledgement = catalog_update
        .and_then(|()| command_succeeded(writer, request_id, None, Vec::new(), Some(response)));
    // A disconnected requester must not prevent other hosts from observing the stop.
    let publication = stopped
        .map_or(Ok(()), |stopped| {
            control::publish_stop_updates(state, &runtime, &stopped, None)
        })
        .and_then(|()| {
            changed_conversation_ref.map_or(Ok(()), |conversation_ref| {
                broadcast_conversation(state, &runtime, &conversation_ref).map(|_| ())
            })
        });
    acknowledgement.and(publication)
}

fn execute_intrinsic(
    runtime: &LocalRuntime,
    command: &IntrinsicCwrCommand,
    requesting_actor_ref: &str,
    conversation_ref: Option<&str>,
    request_id: &str,
    operation_id: Option<&str>,
) -> CwrResult<IntrinsicCwrResponse> {
    if let Some(response) =
        runtime.discovery_as(conversation_ref, requesting_actor_ref, None, command)?
    {
        return Ok(response);
    }
    let module = runtime.bundled_conversation_module_as(
        "cwp://!cwr/actor_integrations/host",
        requesting_actor_ref,
    )?;
    let host = intrinsic_host();
    let dispatch =
        runtime.dispatch_intrinsic_command_as_for_host(requesting_actor_ref, command, &host)?;
    Ok(match dispatch {
        IntrinsicCwrDispatch::Response(response) => response,
        IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ReloadGraph) => {
            match runtime
                .reload_graph_for_connected_actor_validated(requesting_actor_ref, |_, _| Ok(()))
            {
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
            let result = match invocation_ref.as_deref() {
                Some(invocation_ref) => runtime.inspect_invocation(invocation_ref),
                None => conversation_ref.map_or_else(
                    || {
                        Err(
                            "provide an invocation ref or run this command in a conversation"
                                .into(),
                        )
                    },
                    |conversation_ref| {
                        runtime.inspect_latest_invocation_for_conversation(conversation_ref)
                    },
                ),
            };
            match result {
                Ok(report) => intrinsic_cwr_invocation_debug_succeeded(report),
                Err(error) => {
                    intrinsic_cwr_invocation_debug_failed(invocation_ref, error.to_string())
                }
            }
        }
        IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::StopInvocation {
            invocation_ref,
        }) => runtime.stop_intrinsic_invocation_as(
            conversation_ref,
            invocation_ref.as_deref(),
            requesting_actor_ref,
            Some("Stopped from a connected host".into()),
        )?,
        IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ListConversations {
            page,
            archived,
        }) => IntrinsicCwrResponse::ConversationsListed {
            response: cwr::intrinsic_cwr::ConversationListResponse {
                page: module.list(&page, archived, Instant::now() + Duration::from_secs(5))?,
            },
        },
        IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ReadConversation {
            conversation_ref: requested_ref,
            page,
        }) => {
            let conversation_ref =
                requested_ref
                    .as_deref()
                    .or(conversation_ref)
                    .ok_or_else(|| {
                        std::io::Error::other(
                            "provide a conversation ref or run this command in a conversation",
                        )
                    })?;
            IntrinsicCwrResponse::ConversationRead {
                response: cwr::intrinsic_cwr::ConversationReadResponse {
                    conversation_ref: conversation_ref.into(),
                    records: cwr::intrinsic_cwr::ConversationReadRecords::Timeline {
                        page: module.read_command_timeline(
                            conversation_ref,
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
                .or(conversation_ref)
                .ok_or_else(|| {
                    std::io::Error::other(
                        "provide a conversation ref or run this command in a conversation",
                    )
                })?;
            IntrinsicCwrResponse::ConversationBrowsed {
                response: module.browse_with_request_context(
                    conversation_ref,
                    &request,
                    Instant::now() + Duration::from_secs(5),
                    request_id,
                    operation_id,
                )?,
            }
        }
        IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ExportConversation) => {
            module.export(conversation_ref)?
        }
        IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ImportConversation { path }) => {
            module.import(&path, operation_id)?
        }
        IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::ForkConversation {
            message_ref,
        }) => module.fork(
            conversation_ref,
            &message_ref,
            operation_id.ok_or_else(|| std::io::Error::other("fork operation ID is required"))?,
        )?,
        IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::RenameConversation { title }) => {
            module.rename(conversation_ref, &title)?
        }
        IntrinsicCwrDispatch::HostAction(IntrinsicCwrHostAction::SetConversationArchived {
            archived,
        }) => {
            module
                .set_archived_with_stops(conversation_ref, archived, None)?
                .0
        }
    })
}

fn intrinsic_host() -> LocalIntrinsicCwrHost {
    LocalIntrinsicCwrHost {
        capabilities: IntrinsicCwrHostCapabilities {
            reload_graph: true,
            debug_invocation: true,
            stop_invocation: true,
            read_conversations: true,
            rename_conversation: true,
            archive_conversation: true,
            transfer_conversation: true,
        },
        additional_coworkers: Vec::new(),
    }
}

mod archive;
pub(super) use archive::{execute_archive_in_background, ArchiveControl};
