use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use cwr::{
    v2026_09::{
        local::{LocalCatalog, LocalWorkspaceBinding},
        service::{RuntimeServiceCommand, RuntimeServiceEvent, RUNTIME_SERVICE_VERSION},
    },
    CwrRef, CwrResult,
};
use serde::{Deserialize, Serialize};

const WORKSPACE_HANDSHAKE_VERSION: u32 = 1;
const MAX_WORKSPACE_HANDSHAKE_BYTES: usize = 16 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceHandshake {
    version: u32,
    workspace_id: String,
    workspace_path: PathBuf,
    ingress_integration_ref: String,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[expect(
    clippy::enum_variant_names,
    reason = "variant names preserve the resident workspace handshake wire protocol"
)]
enum WorkspaceHandshakeResponse<'a> {
    WorkspaceBound { version: u32 },
    WorkspaceMoved { version: u32 },
    WorkspaceRejected { version: u32, error: &'a str },
}

enum WorkspaceBindingOutcome {
    Bound(String),
    WorkspaceMoved,
}

#[derive(Clone)]
pub(crate) enum SessionSelection {
    Workspace(LocalWorkspaceBinding),
    Configured(crate::configured_runtime::SessionBinding),
    Unbound,
}

pub(crate) struct HostServer {
    _socket: crate::server::socket::OwnedSocket,
}

pub(crate) fn start(
    host_socket_path: &Path,
    runtime_socket_path: &Path,
    runtime_ref: &str,
    state_directory: &Path,
    selection: SessionSelection,
    shutdown_requested: Arc<AtomicBool>,
    activity: crate::idle::Activity,
) -> CwrResult<HostServer> {
    let (socket, listener) = crate::server::socket::OwnedSocket::bind(host_socket_path)?;
    let runtime_socket_path = runtime_socket_path.to_owned();
    let runtime_ref = runtime_ref.to_owned();
    let state_directory = state_directory.to_owned();
    thread::Builder::new()
        .name("cwp-runtime-host-listener".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let Some(consumer) = activity.connect() else {
                    continue;
                };
                let client_activity = activity.clone();
                let runtime_socket_path = runtime_socket_path.clone();
                let runtime_ref = runtime_ref.clone();
                let state_directory = state_directory.clone();
                let selection = selection.clone();
                let shutdown_requested = Arc::clone(&shutdown_requested);
                let _ = thread::Builder::new()
                    .name("cwp-runtime-host-client".into())
                    .spawn(move || {
                        let _consumer = consumer;
                        let closing_stream = stream.try_clone().ok();
                        if let Err(error) = handle_connection(
                            stream,
                            runtime_socket_path,
                            runtime_ref,
                            state_directory,
                            selection,
                            shutdown_requested,
                            client_activity,
                        ) {
                            eprintln!("CWP runtime host session ended: {error}");
                        }
                        if let Some(stream) = closing_stream {
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                        }
                    });
            }
        })?;
    Ok(HostServer { _socket: socket })
}

fn handle_connection(
    mut stream: UnixStream,
    runtime_socket_path: PathBuf,
    runtime_ref: String,
    state_directory: PathBuf,
    selection: SessionSelection,
    shutdown_requested: Arc<AtomicBool>,
    activity: crate::idle::Activity,
) -> CwrResult<()> {
    let binding = match &selection {
        SessionSelection::Configured(expected) => {
            match crate::configured_runtime::bind(&mut stream, expected)? {
                Some(ingress) => WorkspaceBindingOutcome::Bound(ingress),
                None => return Ok(()),
            }
        }
        SessionSelection::Workspace(expected) => bind_workspace(&mut stream, expected)?,
        SessionSelection::Unbound => {
            WorkspaceBindingOutcome::Bound(crate::bridge::HOST_SOCKET_INGRESS_REF.into())
        }
    };
    let ingress_integration_ref = match binding {
        WorkspaceBindingOutcome::Bound(integration_ref) => integration_ref,
        WorkspaceBindingOutcome::WorkspaceMoved => {
            shutdown_requested.store(true, Ordering::SeqCst);
            return Ok(());
        }
    };
    let input = crate::bridge::start_stream_input_reader(stream.try_clone()?)?;
    let output = crate::bridge::output::host_socket_writer(stream)?;
    // Keep the delivery active through output draining before process exit.
    let _delivery = activity.delivery();
    let outcome = (|| {
        let (workspace, default_catalog) = inspect_resident_actor(&runtime_socket_path, None)?;
        let (selected, catalog) = crate::host_session::wait_for_actor(
            workspace,
            Some(&default_catalog.acting_actor.actor_ref),
            &input,
            &output,
            |actor_ref| {
                if matches!(&selection, SessionSelection::Configured(binding) if binding.actor_ref() != actor_ref)
                {
                    return Err(std::io::Error::other(
                        "configured runtime requires its configured acting coworker",
                    )
                    .into());
                }
                inspect_resident_actor(&runtime_socket_path, Some(actor_ref))
                    .map(|(_, catalog)| catalog)
            },
        )?;
        let actor_ref = selected.actor_ref.clone();
        crate::host_session::announce_actor(selected, &catalog, &output)?;
        let performance_database_path = (ingress_integration_ref
            == crate::bridge::WORKLOOP_DESKTOP_INGRESS_REF)
            .then(|| state_directory.join(cwr::LOCAL_METRICS_DATABASE_FILENAME));
        crate::bridge::run_with_input(
            &runtime_socket_path,
            &runtime_ref,
            &ingress_integration_ref,
            Some(&actor_ref),
            performance_database_path,
            input,
            Arc::clone(&output),
        )
    })();
    let output_outcome = crate::bridge::output::finish(&output);
    outcome.and(output_outcome)
}

fn inspect_resident_actor(
    runtime_socket_path: &Path,
    actor_ref: Option<&str>,
) -> CwrResult<(cwr::v2026_09::host::HostWorkspace, LocalCatalog)> {
    let mut stream = UnixStream::connect(runtime_socket_path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request_id = "host-session-inspect-actor";
    serde_json::to_writer(
        &mut stream,
        &RuntimeServiceCommand::InspectActor {
            version: RUNTIME_SERVICE_VERSION,
            request_id: request_id.into(),
            actor_ref: actor_ref.map(str::to_owned),
        },
    )?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "resident closed during actor inspection",
            )
            .into());
        }
        match serde_json::from_str::<RuntimeServiceEvent>(&line)? {
            RuntimeServiceEvent::ActorInspected {
                request_id: returned,
                workspace,
                catalog,
            } if returned == request_id => return Ok((workspace, catalog)),
            RuntimeServiceEvent::CommandFailed {
                request_id: returned,
                error,
            } if returned == request_id => {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error).into())
            }
            _ => {}
        }
    }
}

fn bind_workspace(
    stream: &mut UnixStream,
    expected: &LocalWorkspaceBinding,
) -> CwrResult<WorkspaceBindingOutcome> {
    let outcome = read_workspace_handshake(stream).and_then(|handshake| {
        if handshake.version != WORKSPACE_HANDSHAKE_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "unsupported resident workspace handshake version {}",
                    handshake.version
                ),
            )
            .into());
        }
        let workspace_path = std::fs::canonicalize(&handshake.workspace_path)?;
        if handshake.workspace_id == expected.workspace_id
            && workspace_path != expected.workspace_directory
            && !expected.workspace_directory.exists()
        {
            return Ok(WorkspaceBindingOutcome::WorkspaceMoved);
        }
        if handshake.workspace_id != expected.workspace_id
            || workspace_path != expected.workspace_directory
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "resident runtime owns workspace {} at {}, not workspace {} at {}",
                    expected.workspace_id,
                    expected.workspace_directory.display(),
                    handshake.workspace_id,
                    workspace_path.display()
                ),
            )
            .into());
        }
        let integration = CwrRef::parse(&handshake.ingress_integration_ref).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid resident ingress integration ref: {error}"),
            )
        })?;
        if handshake.ingress_integration_ref.starts_with("~@") || integration.fragment().is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "resident ingress integration ref must be an absolute ref without a fragment",
            )
            .into());
        }
        Ok(WorkspaceBindingOutcome::Bound(
            handshake.ingress_integration_ref,
        ))
    });
    match outcome {
        Ok(WorkspaceBindingOutcome::Bound(ingress_integration_ref)) => {
            write_workspace_handshake_response(
                stream,
                &WorkspaceHandshakeResponse::WorkspaceBound {
                    version: WORKSPACE_HANDSHAKE_VERSION,
                },
            )?;
            Ok(WorkspaceBindingOutcome::Bound(ingress_integration_ref))
        }
        Ok(WorkspaceBindingOutcome::WorkspaceMoved) => {
            write_workspace_handshake_response(
                stream,
                &WorkspaceHandshakeResponse::WorkspaceMoved {
                    version: WORKSPACE_HANDSHAKE_VERSION,
                },
            )?;
            Ok(WorkspaceBindingOutcome::WorkspaceMoved)
        }
        Err(error) => {
            let message = error.to_string();
            let _ = write_workspace_handshake_response(
                stream,
                &WorkspaceHandshakeResponse::WorkspaceRejected {
                    version: WORKSPACE_HANDSHAKE_VERSION,
                    error: &message,
                },
            );
            Err(error)
        }
    }
}

fn read_workspace_handshake(stream: &UnixStream) -> CwrResult<WorkspaceHandshake> {
    let mut input = stream;
    let mut line = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        if input.read(&mut byte)? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "resident host client closed before binding a workspace",
            )
            .into());
        }
        if byte[0] == b'\n' {
            break;
        }
        if line.len() == MAX_WORKSPACE_HANDSHAKE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "resident workspace handshake is too large",
            )
            .into());
        }
        line.push(byte[0]);
    }
    Ok(serde_json::from_slice(&line)?)
}

fn write_workspace_handshake_response(
    stream: &mut UnixStream,
    response: &WorkspaceHandshakeResponse<'_>,
) -> CwrResult<()> {
    serde_json::to_writer(&mut *stream, response)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}
