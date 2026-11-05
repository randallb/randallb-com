use std::{
    collections::BTreeMap,
    fs,
    io::BufReader,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
use std::io::BufRead;

use crate::live_status::LiveStatusRegistry;
use cwr::{
    adapter::AdapterModelOutputMailbox,
    intrinsic_cwr::IntrinsicCwrResponse,
    v2026_09::{
        host::project_conversation,
        local::{LocalAdmission, LocalRuntime, LocalRuntimeEvent, LocalStopOutcome},
        service::{
            CommunicationDestination, RuntimeAdmissionReceipt, RuntimeServiceCommand,
            RuntimeServiceEvent, RUNTIME_SERVICE_VERSION,
        },
    },
    CwrResult,
};

mod attachments;
mod control;
mod drain;
mod ingress;
mod intrinsic;
mod listener;
mod live_output;
mod publication;
mod restart;
mod retirement;
mod scheduler;
mod snapshot;
pub(crate) mod socket;
mod status;
mod writer;

use attachments::{RuntimeAttachment, RuntimeAttachmentState};
use live_output::{
    close_invocations as close_live_invocations, event_conversation_ref,
    observe_runtime_event as observe_live_output, start_live_output_broadcast, LiveOutputRegistry,
    RuntimeLiveOutputPublisher,
};
#[cfg(test)]
use publication::broadcast;
use publication::{
    broadcast_conversation, broadcast_in_publication, lock_broadcast,
    needs_full_conversation_broadcast,
};
use snapshot::write_runtime_snapshot;
use status::{broadcast_turn_statuses, current_unix_ms, invalid, start_live_status_expiry};
#[cfg(test)]
use writer::disconnect_writer;
use writer::{
    client_writer, enable_fork_metadata, enable_metadata_bootstrap, retire_writer,
    upstream_stop_sender, write_event, SharedWriter,
};

const DESKTOP_METADATA_SNAPSHOT_REQUEST_ID: &str = "runtime-bridge-desktop-snapshot";

struct RegisteredCommunicationAdapter {
    _session_ref: String,
    destinations: Vec<CommunicationDestination>,
    writer: SharedWriter,
}

struct RegisteredGateway {
    integration_ref: String,
    approval_fingerprint: String,
    writer: SharedWriter,
    /// Exact ingress event IDs issued through SubmitAction on this writer.
    issued: BTreeMap<String, String>,
}

struct RuntimeServerState {
    generation_restart: Option<(SharedWriter, String)>,
    committed_retirement: Option<cwr::v2026_09::local::LocalResidentRetirement>,
    armed_retirement: Option<cwr::v2026_09::local::LocalResidentRetirement>,
    retirement_allowed: bool,
    controls_finalizing: bool,
    conversation_controls: BTreeMap<String, std::collections::VecDeque<intrinsic::ArchiveControl>>,
    runtime_ref: String,
    runtime: LocalRuntime,
    subscribers: Vec<SharedWriter>,
    host_actors: BTreeMap<String, String>,
    communication_adapters: BTreeMap<String, RegisteredCommunicationAdapter>,
    gateways: BTreeMap<String, RegisteredGateway>,
    attachments: BTreeMap<String, RuntimeAttachment>,
    pending_default_conversation_refs: std::collections::BTreeSet<String>,
    expected_communication_adapters: usize,
    attachment_state_path: PathBuf,
    published_stopped_invocations: std::collections::BTreeSet<String>,
    published_turn_statuses: BTreeMap<String, Vec<cwr::v2026_09::host::ConversationTurnStatus>>,
    live_statuses: LiveStatusRegistry,
    live_outputs: LiveOutputRegistry,
}

struct RuntimeServerShared {
    state: Mutex<RuntimeServerState>,
    broadcast_lock: Mutex<()>,
    graphql_changes: tokio::sync::watch::Sender<()>,
    scheduler: scheduler::ExecutionScheduler,
    activity: crate::idle::Activity,
}

type SharedState = Arc<RuntimeServerShared>;

pub(crate) struct RuntimeServer {
    pub(crate) activity: crate::idle::Activity,
    _socket: socket::OwnedSocket,
    listener: Option<UnixListener>,
    accepting: Arc<std::sync::atomic::AtomicBool>,
    runtime: Option<LocalRuntime>,
    state: SharedState,
}

pub(crate) fn start(
    socket_path: &Path,
    runtime_ref: String,
    runtime: LocalRuntime,
    attachment_state_path: PathBuf,
    expected_communication_adapters: usize,
) -> CwrResult<RuntimeServer> {
    let (socket, listener) = socket::OwnedSocket::bind(socket_path)?;
    let managed_runtime = runtime.clone();
    let RuntimeAttachmentState {
        attachments,
        pending_default_conversation_refs,
    } = attachments::load(&attachment_state_path)?;
    let activity = crate::idle::Activity::default();
    let state = Arc::new(RuntimeServerShared {
        state: Mutex::new(RuntimeServerState {
            generation_restart: None,
            committed_retirement: None,
            armed_retirement: None,
            retirement_allowed: false,
            controls_finalizing: false,
            conversation_controls: BTreeMap::new(),
            runtime_ref,
            runtime,
            subscribers: Vec::new(),
            host_actors: BTreeMap::new(),
            communication_adapters: BTreeMap::new(),
            gateways: BTreeMap::new(),
            attachments,
            pending_default_conversation_refs,
            expected_communication_adapters,
            attachment_state_path,
            published_turn_statuses: BTreeMap::new(),
            published_stopped_invocations: std::collections::BTreeSet::new(),
            live_statuses: LiveStatusRegistry::default(),
            live_outputs: LiveOutputRegistry::default(),
        }),
        broadcast_lock: Mutex::new(()),
        graphql_changes: tokio::sync::watch::channel(()).0,
        scheduler: scheduler::ExecutionScheduler::default(),
        activity: activity.clone(),
    });
    start_live_status_expiry(Arc::clone(&state))?;
    start_live_output_broadcast(Arc::clone(&state))?;
    Ok(RuntimeServer {
        activity,
        _socket: socket,
        listener: Some(listener),
        accepting: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        runtime: Some(managed_runtime),
        state,
    })
}

impl RuntimeServer {
    pub(crate) fn graphql_changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.state.graphql_changes.subscribe()
    }

    pub(crate) fn graphql_admission_publisher(&self) -> crate::graphql::AdmissionPublisher {
        let state = Arc::downgrade(&self.state);
        Arc::new(move |admission| {
            let state = state
                .upgrade()
                .ok_or_else(|| std::io::Error::other("runtime is unavailable"))?;
            let runtime = lock_state(&state)?.runtime.clone();
            broadcast_conversation(&state, &runtime, admission.conversation_ref())?;
            if admission.invocation_ref().is_some() {
                scheduler::start(&state)?;
            }
            broadcast_turn_statuses(&state, &runtime)?;
            attachments::request_deliveries(&state, admission.conversation_ref())?;
            if admission
                .conversation
                .messages
                .first()
                .is_some_and(|message| message.message_ref == admission.record.source_message_ref)
            {
                attachments::request_defaults(&state, admission)?;
            }
            Ok(())
        })
    }

    pub(crate) fn should_exit(&self, auto_exit: bool) -> CwrResult<bool> {
        self.activity.should_exit(auto_exit, || {
            lock_state(&self.state)?.runtime.has_unfinished_work()
        })
    }

    pub(crate) fn recover_pending(&self) -> CwrResult<()> {
        scheduler::start(&self.state)
    }

    pub(crate) fn shutdown(&mut self) -> CwrResult<Vec<String>> {
        self.stop_accepting();
        self.freeze_retirement_control()?;
        self.state.scheduler.stop();
        self.runtime.take().map_or_else(
            || Ok(Vec::new()),
            |runtime| runtime.shutdown(Duration::from_secs(5)),
        )
    }
}

fn recover_pending(state: SharedState) -> CwrResult<()> {
    scheduler::start(&state)
}

impl Drop for RuntimeServer {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            eprintln!("cwp-runtime could not finish managed shutdown: {error}");
        }
    }
}

fn handle_connection(state: SharedState, stream: UnixStream) -> CwrResult<()> {
    let writer = client_writer(stream.try_clone()?)?;
    let runtime_ref = lock_state(&state)?.runtime_ref.clone();
    write_event(&writer, &RuntimeServiceEvent::Ready { runtime_ref })?;
    let outcome = handle_connection_commands(&state, &writer, stream);
    unsubscribe(&state, &writer);
    outcome
}

fn handle_connection_commands(
    state: &SharedState,
    writer: &SharedWriter,
    stream: UnixStream,
) -> CwrResult<()> {
    let mut subscribed = false;
    let mut input = BufReader::new(stream);
    while let Some(line) = crate::jsonl::read_delimited_line(&mut input)? {
        let command: RuntimeServiceCommand = serde_json::from_str(&line)?;
        let request_id = command.request_id().to_owned();
        let capability_probe = matches!(
            &command,
            RuntimeServiceCommand::Drain { operation, .. }
                if operation.starts_with("supports-")
        );
        let result = if !subscribed
            && matches!(
                &command,
                RuntimeServiceCommand::Snapshot { version, .. }
                    if *version == RUNTIME_SERVICE_VERSION
            ) {
            let RuntimeServiceCommand::Snapshot {
                request_id,
                metadata_only,
                ..
            } = command
            else {
                unreachable!("the initial command was matched as a snapshot")
            };
            if metadata_only {
                enable_metadata_bootstrap(writer);
                if request_id == DESKTOP_METADATA_SNAPSHOT_REQUEST_ID {
                    enable_fork_metadata(writer);
                }
            }
            let result = snapshot::write_initial_runtime_snapshot(state, writer, request_id);
            if result.is_ok() {
                subscribed = true;
            }
            result
        } else if !subscribed
            && (capability_probe
                || matches!(
                    &command,
                    RuntimeServiceCommand::RetireConfiguredResident { .. }
                        | RuntimeServiceCommand::ArmConfiguredResident { .. }
                ))
        {
            execute_command(state, writer, command)
        } else {
            if !subscribed {
                subscribe(state, writer)?;
                subscribed = true;
            }
            execute_command(state, writer, command)
        };
        if let Err(error) = result {
            write_event(
                writer,
                &RuntimeServiceEvent::CommandFailed {
                    request_id,
                    error: error.to_string(),
                },
            )?;
        }
    }
    Ok(())
}

include!("server/actor_command.rs");
include!("server/command.rs");
include!("server/gateway_command.rs");
include!("server/text_command.rs");
include!("server/upstream.rs");

#[expect(
    clippy::too_many_arguments,
    reason = "The validated live-status command remains explicit at the service boundary."
)]
fn set_conversation_live_status(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    conversation_ref: String,
    subject_actor_ref: String,
    publisher_session_id: String,
    sequence: u64,
    target_actor_refs: Vec<String>,
    status: Option<cwr::v2026_09::host::ConversationLiveStatusName>,
    lease_ms: u64,
) -> CwrResult<()> {
    let _publication = lock_broadcast(state)?;
    let statuses = {
        let mut state = lock_state(state)?;
        let conversation = state
            .runtime
            .conversation(&conversation_ref)?
            .ok_or_else(|| {
                std::io::Error::other(format!("unknown conversation {conversation_ref}"))
            })?;
        if !conversation
            .participant_actor_refs
            .contains(&subject_actor_ref)
            || target_actor_refs
                .iter()
                .any(|actor_ref| !conversation.participant_actor_refs.contains(actor_ref))
        {
            return invalid("live status actors must participate in the conversation");
        }
        state.live_statuses.update(
            &conversation_ref,
            subject_actor_ref,
            publisher_session_id,
            sequence,
            target_actor_refs,
            status,
            lease_ms,
            current_unix_ms(),
        )
    };
    command_succeeded(writer, request_id, None, Vec::new(), None)?;
    if let Some(statuses) = statuses {
        broadcast_in_publication(
            state,
            RuntimeServiceEvent::ConversationLiveStatusSnapshot {
                conversation_ref,
                statuses,
            },
        )?;
    }
    Ok(())
}

fn accept_admission(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    admission: LocalAdmission,
    attachment_ref: Option<String>,
) -> CwrResult<()> {
    let publication = lock_broadcast(state)?;
    let runtime = lock_state(state)?.runtime.clone();
    let metadata_writer = writer::uses_metadata_bootstrap(writer);
    let conversation = if !metadata_writer || needs_full_conversation_broadcast(state)? {
        runtime
            .conversation_host()
            .conversation(admission.conversation_ref())?
    } else {
        runtime
            .conversation_host()
            .metadata(admission.conversation_ref())?
    }
    .unwrap_or_else(|| project_conversation(&admission.conversation));
    let mut receipt = RuntimeAdmissionReceipt {
        record: admission.record.clone(),
        conversation: conversation.clone(),
        attachment_ref,
    };
    if metadata_writer {
        receipt.conversation.messages.clear();
        receipt.conversation.activity.clear();
    }
    if admission.invocation_ref().is_some() {
        scheduler::start(state)?;
    }
    command_succeeded(writer, request_id, Some(receipt), Vec::new(), None)?;
    broadcast_in_publication(
        state,
        RuntimeServiceEvent::ConversationUpsert { conversation },
    )?;
    drop(publication);
    broadcast_turn_statuses(state, &runtime)?;
    attachments::request_deliveries(state, &admission.record.conversation_ref)?;
    if admission
        .conversation
        .messages
        .first()
        .is_some_and(|message| message.message_ref == admission.record.source_message_ref)
    {
        attachments::request_defaults(state, &admission)?;
    }
    Ok(())
}

fn broadcast_execution_event(
    state: &SharedState,
    conversation_ref: &str,
    event: &LocalRuntimeEvent,
) -> CwrResult<()> {
    let LocalRuntimeEvent::Terminal {
        target_actor_ref,
        response,
        ..
    } = event
    else {
        return Ok(());
    };
    broadcast_execution_terminal(state, conversation_ref, target_actor_ref, response.clone())
}

fn broadcast_execution_terminal(
    state: &SharedState,
    conversation_ref: &str,
    target_actor_ref: &str,
    response: Box<cwr::v2026_09::invocation::ActionResponse>,
) -> CwrResult<()> {
    let _publication = lock_broadcast(state)?;
    if response.status.is_stopped()
        && !lock_state(state)?
            .published_stopped_invocations
            .insert(response.invocation_ref.clone())
    {
        return Ok(());
    }
    broadcast_in_publication(
        state,
        RuntimeServiceEvent::ConversationExecutionFinished {
            conversation_ref: conversation_ref.into(),
            target_actor_ref: target_actor_ref.into(),
            response,
        },
    )
}

fn broadcast_execution_failure(
    state: &SharedState,
    conversation_ref: &str,
    invocation_ref: &str,
    target_actor_ref: &str,
    error: &str,
) -> CwrResult<()> {
    let _publication = lock_broadcast(state)?;
    broadcast_in_publication(
        state,
        RuntimeServiceEvent::ConversationExecutionFailed {
            conversation_ref: conversation_ref.into(),
            invocation_ref: invocation_ref.into(),
            target_actor_ref: target_actor_ref.into(),
            error: error.into(),
        },
    )
}

fn command_succeeded(
    writer: &SharedWriter,
    request_id: String,
    admission: Option<RuntimeAdmissionReceipt>,
    stopped_invocation_refs: Vec<String>,
    intrinsic_response: Option<IntrinsicCwrResponse>,
) -> CwrResult<()> {
    write_event(
        writer,
        &RuntimeServiceEvent::CommandSucceeded {
            request_id,
            admission: admission.map(Box::new),
            stopped_invocation_refs,
            intrinsic_response: intrinsic_response.map(Box::new),
            host_exchange: None,
        },
    )
}

fn subscribe(state: &SharedState, writer: &SharedWriter) -> CwrResult<()> {
    let mut state = lock_state(state)?;
    if !state
        .subscribers
        .iter()
        .any(|subscriber| Arc::ptr_eq(subscriber, writer))
    {
        state.subscribers.push(Arc::clone(writer));
    }
    Ok(())
}

fn unsubscribe(state: &SharedState, writer: &SharedWriter) {
    retire_writer(writer);
    restart::cancel_disconnected(state, writer);
    if let Ok(mut state) = state.state.lock() {
        let owner = upstream_owner(writer);
        state.host_actors.remove(&owner);
        let _ = state.runtime.disconnect_upstream_provider(&owner);
    }
    if let Ok(mut state) = state.state.lock() {
        state
            .subscribers
            .retain(|subscriber| !Arc::ptr_eq(subscriber, writer));
        state
            .communication_adapters
            .retain(|_, adapter| !Arc::ptr_eq(&adapter.writer, writer));
        state
            .gateways
            .retain(|_, gateway| !Arc::ptr_eq(&gateway.writer, writer));
    }
    if let Ok(runtime) = lock_state(state).map(|state| state.runtime.clone()) {
        if let Ok(catalog) = runtime.catalog() {
            if let Ok(_publication) = lock_broadcast(state) {
                let _ = broadcast_in_publication(
                    state,
                    RuntimeServiceEvent::CatalogUpdated { catalog },
                );
            }
        }
    }
}

fn lock_state(state: &SharedState) -> CwrResult<MutexGuard<'_, RuntimeServerState>> {
    state
        .state
        .lock()
        .map_err(|_| std::io::Error::other("runtime server state was poisoned").into())
}

#[cfg(test)]
mod tests;
