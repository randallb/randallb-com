use std::{
    io::{self, BufReader, Write},
    net::Shutdown,
    os::fd::AsRawFd,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

#[cfg(test)]
use std::io::BufRead;

use cwr::{
    v2026_09::{
        host::{
            project_host_actor, project_host_catalog, ConversationSource,
            ConversationSourceCapabilities, HostCommand, HostCommandReceipt, HostEvent,
        },
        service::{RuntimeServiceCommand, RuntimeServiceEvent, RUNTIME_SERVICE_VERSION},
    },
    CwrResult,
};

const INTERNAL_SNAPSHOT_REQUEST_ID: &str = "runtime-bridge-snapshot";
const INTERNAL_DESKTOP_SNAPSHOT_REQUEST_ID: &str = "runtime-bridge-desktop-snapshot";
const INTERNAL_TRANSFER_PROBE_REQUEST_ID: &str = "runtime-bridge-transfer-probe";
const INTERNAL_BUILD_IDENTITY_REQUEST_ID: &str = "runtime-bridge-build-identity";
const INTERNAL_FORK_PROBE_REQUEST_ID: &str = "runtime-bridge-fork-probe";
const HOST_INPUT_QUEUE_CAPACITY: usize = 64;
pub(crate) const HOST_SOCKET_INGRESS_REF: &str =
    "cwp://org.coworkerprotocol.local/integrations/host-socket";
pub(crate) const WORKLOOP_DESKTOP_INGRESS_REF: &str =
    "cwp://org.coworkerprotocol.local/integrations/workloop-desktop";

mod drain;
pub(crate) mod output;
mod pending;
mod probes;
mod projection;

use output::SharedHostWriter;
use pending::SharedPendingRequests;
use projection::{root_conversation_snapshot, root_conversation_upsert};

pub(crate) fn run(
    socket_path: &Path,
    runtime_ref: &str,
    ingress_integration_ref: &str,
    bound_actor_ref: Option<&str>,
    performance_database_path: Option<PathBuf>,
) -> CwrResult<()> {
    run_with_input(
        socket_path,
        runtime_ref,
        ingress_integration_ref,
        bound_actor_ref,
        performance_database_path,
        start_input_reader()?,
        output::host_writer()?,
    )
}

pub(crate) enum HostInput {
    Line(String),
    Closed(CwrResult<()>),
    Disconnected(CwrResult<()>),
}

pub(crate) fn run_with_input(
    socket_path: &Path,
    runtime_ref: &str,
    ingress_integration_ref: &str,
    bound_actor_ref: Option<&str>,
    performance_database_path: Option<PathBuf>,
    input: Receiver<HostInput>,
    output: SharedHostWriter,
) -> CwrResult<()> {
    let stream = UnixStream::connect(socket_path)?;
    let writer = Arc::new(Mutex::new(stream.try_clone()?));
    let pending = pending::registry();
    let event_reader = start_event_reader(
        stream,
        Arc::clone(&output),
        Arc::clone(&pending),
        runtime_ref.into(),
        ingress_integration_ref == WORKLOOP_DESKTOP_INGRESS_REF,
        performance_database_path,
    )?;
    probes::query_resident(&writer)?;
    write_command(
        &writer,
        &RuntimeServiceCommand::Snapshot {
            version: RUNTIME_SERVICE_VERSION,
            request_id: if ingress_integration_ref == WORKLOOP_DESKTOP_INGRESS_REF {
                INTERNAL_DESKTOP_SNAPSHOT_REQUEST_ID.into()
            } else {
                INTERNAL_SNAPSHOT_REQUEST_ID.into()
            },
            metadata_only: uses_metadata_bootstrap(ingress_integration_ref),
        },
    )?;
    if let Some(actor) = bound_actor_ref {
        write_command(
            &writer,
            &RuntimeServiceCommand::InspectActor {
                version: RUNTIME_SERVICE_VERSION,
                request_id: "upstream-bind-host".into(),
                actor_ref: Some(actor.into()),
            },
        )?;
    }
    let source_ref = source_ref(runtime_ref);
    loop {
        match event_reader.try_recv() {
            Ok(outcome) => return outcome,
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(io::Error::other("runtime event reader exited unexpectedly").into())
            }
        }
        let line = match input.recv_timeout(Duration::from_millis(25)) {
            Ok(HostInput::Line(line)) => line,
            Ok(HostInput::Closed(outcome)) => {
                let drain_outcome =
                    drain::pending_requests(&event_reader, &writer, &output, &pending, false);
                if let Ok(writer) = writer.lock() {
                    let _ = writer.shutdown(Shutdown::Both);
                }
                return outcome.and(drain_outcome);
            }
            Ok(HostInput::Disconnected(outcome)) => {
                let drain_outcome =
                    drain::pending_requests(&event_reader, &writer, &output, &pending, true);
                if let Ok(writer) = writer.lock() {
                    let _ = writer.shutdown(Shutdown::Both);
                }
                return outcome.and(drain_outcome);
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("host input reader exited unexpectedly").into())
            }
        };
        let Some(command) = translate_host_command(
            serde_json::from_str(&line)?,
            &source_ref,
            ingress_integration_ref,
            bound_actor_ref,
            &output,
            &pending,
        )?
        else {
            continue;
        };
        write_command(&writer, &command)?;
    }
}

fn uses_metadata_bootstrap(ingress_integration_ref: &str) -> bool {
    ingress_integration_ref == WORKLOOP_DESKTOP_INGRESS_REF
        || ingress_integration_ref == "cwp://org.coworkerprotocol.local/integrations/tui"
}

include!("bridge/command.rs");

pub(crate) fn start_input_reader() -> CwrResult<Receiver<HostInput>> {
    let (sender, receiver) = mpsc::sync_channel(HOST_INPUT_QUEUE_CAPACITY);
    thread::Builder::new()
        .name("cwp-runtime-host-input".into())
        .spawn(move || {
            let mut input = BufReader::new(io::stdin().lock());
            loop {
                match crate::jsonl::read_delimited_line(&mut input) {
                    Ok(Some(line)) => {
                        if sender.send(HostInput::Line(line)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(HostInput::Closed(Err(error.into())));
                        return;
                    }
                }
            }
            let _ = sender.send(HostInput::Closed(Ok(())));
        })?;
    Ok(receiver)
}

pub(crate) fn start_stream_input_reader(stream: UnixStream) -> CwrResult<Receiver<HostInput>> {
    let (sender, receiver) = mpsc::sync_channel(HOST_INPUT_QUEUE_CAPACITY);
    thread::Builder::new()
        .name("cwp-runtime-host-socket-input".into())
        .spawn(move || {
            let mut input = BufReader::new(stream);
            loop {
                match crate::jsonl::read_delimited_line(&mut input) {
                    Ok(Some(line)) => {
                        if sender.send(HostInput::Line(line)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {
                        let closed = if socket_peer_disconnected(input.get_ref()) {
                            HostInput::Disconnected(Ok(()))
                        } else {
                            HostInput::Closed(Ok(()))
                        };
                        let _ = sender.send(closed);
                        return;
                    }
                    Err(error) => {
                        let _ = sender.send(HostInput::Disconnected(Err(error.into())));
                        return;
                    }
                }
            }
        })?;
    Ok(receiver)
}

fn socket_peer_disconnected(stream: &UnixStream) -> bool {
    // A zero-length send changes no protocol bytes, but distinguishes a peer
    // that only closed its write half from one that closed the whole socket.
    // SAFETY: The file descriptor belongs to `stream`; a zero-length send does
    // not dereference the null buffer pointer.
    let result = unsafe { libc::send(stream.as_raw_fd(), std::ptr::null(), 0, libc::MSG_NOSIGNAL) };
    result < 0
}

fn start_event_reader(
    stream: UnixStream,
    output: SharedHostWriter,
    pending: SharedPendingRequests,
    runtime_ref: String,
    advertise_fork_capability: bool,
    performance_database_path: Option<PathBuf>,
) -> CwrResult<Receiver<CwrResult<()>>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("cwp-runtime-host-events".into())
        .spawn(move || {
            let _ = sender.send(read_runtime_events(
                stream,
                output,
                pending,
                runtime_ref,
                advertise_fork_capability,
                performance_database_path,
            ));
        })?;
    Ok(receiver)
}

#[expect(
    clippy::too_many_lines,
    reason = "The exhaustive runtime-event projection keeps protocol terminal handling together."
)]
fn read_runtime_events(
    stream: UnixStream,
    output: SharedHostWriter,
    pending: SharedPendingRequests,
    runtime_ref: String,
    advertise_fork_capability: bool,
    performance_database_path: Option<PathBuf>,
) -> CwrResult<()> {
    let source_ref = source_ref(&runtime_ref);
    let performance_database_path =
        performance_database_path.and_then(|path| path.into_os_string().into_string().ok());
    let mut stream = BufReader::new(stream);
    let mut runtime_ready = false;
    let mut build_identity = None;
    let mut fork_supported = false;
    while let Some(line) = crate::jsonl::read_unbounded_delimited_line(&mut stream)? {
        let event = serde_json::from_str::<RuntimeServiceEvent>(&line)?;
        let (host_event, completed_request_id) = match event {
            RuntimeServiceEvent::RuntimeRestarting { generation } => {
                // The host writer acknowledges the complete frame before teardown.
                write_host_event(&output, &HostEvent::RuntimeRestarting { generation })?;
                let _ = stream.get_ref().shutdown(Shutdown::Both);
                return Ok(());
            }
            RuntimeServiceEvent::UpstreamRegistered {
                request_id,
                provider_session_ref,
            } => {
                if pending::claim_terminal(&pending, &request_id)?.is_none() {
                    continue;
                }
                (
                    Some(HostEvent::UpstreamRegistered {
                        request_id: request_id.clone(),
                        provider_session_ref,
                    }),
                    Some(request_id),
                )
            }
            RuntimeServiceEvent::Upstream { event } => (Some(HostEvent::Upstream { event }), None),
            RuntimeServiceEvent::BuildIdentity {
                request_id,
                identity,
            } if request_id == INTERNAL_BUILD_IDENTITY_REQUEST_ID => {
                build_identity = Some(identity);
                (None, None)
            }
            RuntimeServiceEvent::BuildIdentity { .. } => (None, None),
            RuntimeServiceEvent::CommandFailed { request_id, .. }
                if request_id == INTERNAL_BUILD_IDENTITY_REQUEST_ID =>
            {
                (None, None)
            }
            RuntimeServiceEvent::Ready { .. } => {
                runtime_ready = true;
                (None, None)
            }
            RuntimeServiceEvent::DrainStatus { request_id, .. }
                if request_id == INTERNAL_FORK_PROBE_REQUEST_ID =>
            {
                fork_supported = true;
                (None, None)
            }
            RuntimeServiceEvent::DrainStatus { request_id, .. }
                if request_id == INTERNAL_TRANSFER_PROBE_REQUEST_ID =>
            {
                if !runtime_ready {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "runtime transfer probe completed before the ready event",
                    )
                    .into());
                }
                (
                    Some(HostEvent::Ready {
                        sources: vec![ConversationSource {
                            source_ref: source_ref.clone(),
                            runtime_ref: runtime_ref.clone(),
                            display_name: "Local workspace".into(),
                            capabilities: ConversationSourceCapabilities {
                                can_rename: true,
                                can_archive: true,
                                can_transfer: true,
                                can_fork: advertise_fork_capability && fork_supported,
                                can_read_timeline: true,
                                ..ConversationSourceCapabilities::controllable()
                            },
                            performance_database_path: performance_database_path.clone(),
                            build_identity: advertise_fork_capability
                                .then(|| build_identity.clone())
                                .flatten(),
                        }],
                    }),
                    None,
                )
            }
            RuntimeServiceEvent::CatalogUpdated { catalog } => (
                Some(HostEvent::CatalogUpdated {
                    actor: project_host_actor(&catalog.acting_actor),
                    catalog: project_host_catalog(&catalog),
                }),
                None,
            ),
            RuntimeServiceEvent::ActorInspected { .. } => (None, None),
            RuntimeServiceEvent::ConversationSnapshot { conversations } => (
                Some(root_conversation_snapshot(&source_ref, conversations)),
                None,
            ),
            RuntimeServiceEvent::RootConversationsPage { request_id, page } => {
                let Some(event) = project_root_conversations_page(
                    &pending,
                    &source_ref,
                    request_id.clone(),
                    page,
                )?
                else {
                    continue;
                };
                (Some(event), Some(request_id))
            }
            RuntimeServiceEvent::ConversationRecordsPage { request_id, page } => {
                if pending::claim_terminal(&pending, &request_id)?.is_none() {
                    continue;
                }
                (
                    Some(HostEvent::ConversationRecordsPage {
                        request_id: request_id.clone(),
                        source_ref: source_ref.clone(),
                        page,
                    }),
                    Some(request_id),
                )
            }
            RuntimeServiceEvent::VisualizationRead {
                request_id,
                artifact,
            } => {
                if pending::claim_terminal(&pending, &request_id)?.is_none() {
                    continue;
                }
                (
                    Some(HostEvent::VisualizationRead {
                        request_id: request_id.clone(),
                        source_ref: source_ref.clone(),
                        artifact,
                    }),
                    Some(request_id),
                )
            }
            RuntimeServiceEvent::ConversationUpsert { conversation } => {
                (root_conversation_upsert(&source_ref, conversation), None)
            }
            RuntimeServiceEvent::ConversationLiveOutput {
                conversation_ref,
                output,
            } => (
                Some(HostEvent::ConversationLiveOutput {
                    source_ref: source_ref.clone(),
                    conversation_ref,
                    output,
                }),
                None,
            ),
            RuntimeServiceEvent::ConversationLiveStatusSnapshot {
                conversation_ref,
                statuses,
            } => (
                Some(HostEvent::ConversationLiveStatusSnapshot {
                    source_ref: source_ref.clone(),
                    conversation_ref,
                    statuses,
                }),
                None,
            ),
            RuntimeServiceEvent::ConversationTurnStatusSnapshot {
                conversation_ref,
                turns,
            } => (
                Some(HostEvent::ConversationTurnStatusSnapshot {
                    source_ref: source_ref.clone(),
                    conversation_ref,
                    turns,
                }),
                None,
            ),
            RuntimeServiceEvent::ConversationExecutionFinished {
                conversation_ref,
                target_actor_ref,
                response,
            } => (
                Some(HostEvent::ConversationExecutionFinished {
                    source_ref: source_ref.clone(),
                    conversation_ref,
                    target_actor_ref,
                    response,
                }),
                None,
            ),
            RuntimeServiceEvent::ConversationExecutionFailed {
                conversation_ref,
                invocation_ref,
                target_actor_ref,
                error,
            } => (
                Some(HostEvent::ConversationExecutionFailed {
                    source_ref: source_ref.clone(),
                    conversation_ref,
                    invocation_ref,
                    target_actor_ref,
                    error,
                }),
                None,
            ),
            RuntimeServiceEvent::CommandSucceeded {
                request_id,
                intrinsic_response: Some(response),
                ..
            } => {
                let Some(_) = pending::claim_terminal(&pending, &request_id)? else {
                    continue;
                };
                (
                    Some(HostEvent::IntrinsicResponse {
                        request_id: request_id.clone(),
                        response,
                    }),
                    Some(request_id),
                )
            }
            RuntimeServiceEvent::CommandSucceeded {
                request_id,
                admission,
                host_exchange,
                stopped_invocation_refs,
                ..
            } if !is_internal_snapshot_request_id(&request_id) => {
                let Some(claim) = pending::claim_terminal(&pending, &request_id)? else {
                    continue;
                };
                let conversation_ref = host_exchange
                    .as_ref()
                    .map(|exchange| exchange.conversation.conversation_ref.clone())
                    .or_else(|| {
                        admission
                            .as_ref()
                            .map(|admission| admission.record.conversation_ref.clone())
                    })
                    .or(claim.conversation_ref)
                    .unwrap_or_else(|| runtime_ref.clone());
                (
                    Some(HostEvent::CommandSucceeded {
                        request_id: request_id.clone(),
                        receipt: HostCommandReceipt {
                            attachment_ref: admission
                                .as_ref()
                                .and_then(|a| a.attachment_ref.clone()),
                            conversation_ref,
                            message_ref: admission
                                .as_ref()
                                .map(|admission| admission.record.source_message_ref.clone())
                                .or_else(|| {
                                    host_exchange
                                        .as_ref()
                                        .map(|exchange| exchange.request_message_ref.clone())
                                }),
                            invocation_ref: admission
                                .and_then(|admission| admission.record.invocation_ref.clone()),
                            stopped_invocation_refs,
                        },
                    }),
                    Some(request_id),
                )
            }
            RuntimeServiceEvent::CommandFailed { request_id, .. }
                if request_id == INTERNAL_FORK_PROBE_REQUEST_ID =>
            {
                fork_supported = false;
                (None, None)
            }
            RuntimeServiceEvent::CommandFailed { request_id, .. }
                if request_id == INTERNAL_TRANSFER_PROBE_REQUEST_ID =>
            {
                if !runtime_ready {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "runtime transfer probe completed before the ready event",
                    )
                    .into());
                }
                (
                    Some(HostEvent::Ready {
                        sources: vec![ConversationSource {
                            source_ref: source_ref.clone(),
                            runtime_ref: runtime_ref.clone(),
                            display_name: "Local workspace".into(),
                            capabilities: ConversationSourceCapabilities {
                                can_rename: true,
                                can_archive: true,
                                can_transfer: false,
                                can_fork: false,
                                can_read_timeline: true,
                                ..ConversationSourceCapabilities::controllable()
                            },
                            performance_database_path: performance_database_path.clone(),
                            build_identity: advertise_fork_capability
                                .then(|| build_identity.clone())
                                .flatten(),
                        }],
                    }),
                    None,
                )
            }
            RuntimeServiceEvent::CommandFailed { request_id, error } => {
                let Some(_) = pending::claim_terminal(&pending, &request_id)? else {
                    continue;
                };
                (
                    Some(HostEvent::CommandFailed {
                        request_id: request_id.clone(),
                        error,
                    }),
                    Some(request_id),
                )
            }
            _ => (None, None),
        };
        if let Some(event) = host_event {
            write_host_event(&output, &event)?;
        }
        if let Some(request_id) = completed_request_id {
            pending::complete_terminal(&pending, &request_id)?;
        }
    }
    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "runtime service connection closed",
    )
    .into())
}

fn is_internal_snapshot_request_id(request_id: &str) -> bool {
    matches!(
        request_id,
        INTERNAL_SNAPSHOT_REQUEST_ID | INTERNAL_DESKTOP_SNAPSHOT_REQUEST_ID
    )
}

fn project_root_conversations_page(
    pending: &SharedPendingRequests,
    source_ref: &str,
    request_id: String,
    mut page: cwr::v2026_09::conversation::ConversationSummaryPage,
) -> CwrResult<Option<HostEvent>> {
    let Some(claim) = pending::claim_terminal(pending, &request_id)? else {
        return Ok(None);
    };
    if !claim.include_sidebar_metadata {
        page.revision.clear();
        page.membership_revision.clear();
        for summary in &mut page.summaries {
            summary.completed_work.clear();
        }
    }
    if !claim.include_completed_work_refs {
        for summary in &mut page.summaries {
            summary.completed_work_refs.clear();
        }
    }
    Ok(Some(HostEvent::RootConversationsPage {
        request_id,
        source_ref: source_ref.into(),
        page,
    }))
}

include!("bridge/validation.rs");

#[cfg(test)]
mod tests;
