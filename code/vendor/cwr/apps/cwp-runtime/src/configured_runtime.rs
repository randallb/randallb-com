//! Configured workspace selection at the host-stdio boundary.
use std::{
    ffi::OsString,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
};

use cwr::{
    v2026_09::{
        host::{project_host_actor, HostCommand, HostEvent, HostWorkspace},
        local::{LocalWorkspaceComposition, PreparedConfiguredRuntime},
    },
    CwrResult,
};
use serde::{Deserialize, Serialize};

use crate::config::RuntimeConfig;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionBinding {
    workspace_path: PathBuf,
    #[serde(rename = "loop_name")]
    runtime_name: String,
    fingerprint: String,
    acting_actor_ref: String,
}

impl SessionBinding {
    pub(crate) fn new(prepared: &PreparedConfiguredRuntime) -> Self {
        Self {
            workspace_path: prepared.selected().workspace_path().into(),
            runtime_name: prepared.selected().name.clone(),
            fingerprint: prepared.fingerprint.clone(),
            acting_actor_ref: prepared.acting_actor_ref.clone(),
        }
    }

    pub(crate) fn actor_ref(&self) -> &str {
        &self.acting_actor_ref
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Handshake {
    version: u32,
    selection: SessionBinding,
    ingress: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    ensure_only: bool,
}

pub(crate) fn run(workspace: &Path, args: Vec<OsString>) -> CwrResult<()> {
    let [name, operation, rest @ ..] = args.as_slice() else {
        return Err(usage().into());
    };
    let name = name.to_str().ok_or_else(usage)?;
    // Stop and drain address the existing owner even if authored files changed.
    if operation == "--stop-resident" && rest.is_empty() {
        let directory =
            cwr::v2026_09::local::configured_runtime_runtime_directory(workspace, name)?;
        return cwr::v2026_09::local::stop_local_resident(
            &directory.join("resident-runtime.pid"),
            &directory.join("host.sock"),
        );
    }
    if operation == "--observe-resident" {
        let [expected] = rest else {
            return Err(usage().into());
        };
        let directory =
            cwr::v2026_09::local::configured_runtime_runtime_directory(workspace, name)?;
        return print_observation(&directory, expected.to_str().ok_or_else(usage)?);
    }
    if operation == "--drain-resident" {
        let [operation] = rest else {
            return Err(usage().into());
        };
        let directory =
            cwr::v2026_09::local::configured_runtime_runtime_directory(workspace, name)?;
        return crate::drain_client::run_for_socket(
            directory.join("runtime.sock"),
            directory,
            operation.to_str().ok_or_else(usage)?,
        );
    }
    if operation == "--exit-resident-when-idle" && rest.is_empty() {
        let directory =
            cwr::v2026_09::local::configured_runtime_runtime_directory(workspace, name)?;
        return crate::retire_client::run_socket(&directory.join("runtime.sock"));
    }
    if operation == "--ensure-resident-environment" && rest.is_empty() {
        let directory =
            cwr::v2026_09::local::configured_runtime_runtime_directory(workspace, name)?;
        return crate::resident_environment::ensure(
            &directory,
            &directory.join("runtime.sock"),
            &directory.join("host.sock"),
        );
    }
    let composition = LocalWorkspaceComposition::load(workspace)?;
    let selected = composition.configured_runtime(name)?;
    let prepared = selected.prepare_resident()?;
    if operation == "--resident" && rest.is_empty() {
        prepared.create_resident_directories()?;
        let config = runtime_config(prepared)?;
        return crate::run_runtime(
            &config,
            config.configured_actor_ref()?,
            crate::RuntimeMode::Service,
            None,
            None,
            None,
        );
    }
    if operation == "--ensure-resident" {
        let ingress = match rest {
            [] => crate::HOST_STDIO_INGRESS_REF,
            [flag, ingress] if flag == "--ingress" => ingress.to_str().ok_or_else(usage)?,
            _ => return Err(usage().into()),
        };
        // Use the same startup lock, configuration fingerprint and actor binding
        // as host clients, without opening a host command session.
        drop(connect(&prepared, ingress, true)?);
        return Ok(());
    }
    if operation != "--host-stdio" {
        return Err(usage().into());
    }
    let (acting_actor_only, ingress) = match rest {
        [] => (false, crate::HOST_STDIO_INGRESS_REF),
        [flag] if flag == "--acting-actor-only" => (true, crate::HOST_STDIO_INGRESS_REF),
        [flag, ingress] if flag == "--ingress" => (false, ingress.to_str().ok_or_else(usage)?),
        [actor_flag, ingress_flag, ingress]
            if actor_flag == "--acting-actor-only" && ingress_flag == "--ingress" =>
        {
            (true, ingress.to_str().ok_or_else(usage)?)
        }
        _ => return Err(usage().into()),
    };
    run_client(prepared, ingress, acting_actor_only)
}

fn usage() -> io::Error {
    io::Error::other("usage: cwp-runtime <WORKSPACE.toml> --runtime <name> <--ensure-resident [--ingress <ref>]|--host-stdio [--acting-actor-only] [--ingress <ref>]|--stop-resident|--exit-resident-when-idle|--drain-resident <operation>|--observe-resident <expected-runtime-ref>>; configured-runtime-control also accepts --resident-autostart-state, --set-resident-autostart-paused <true|false>, --arm-retirement <known-operation-json>, --retire-resident <known-operation-json>, --retirement-result <known-operation-json>")
}

pub(crate) fn only_name(workspace: &Path) -> CwrResult<()> {
    let composition = only_composition(workspace, "--only-runtime-name")?;
    let selected = &composition.runtimes[0];
    println!("{}", selected.name);
    Ok(())
}

#[derive(Serialize)]
struct OnlyRuntimeSelection {
    runtime_ref: String,
    working_directory: PathBuf,
    name: String,
    runtime_directory: PathBuf,
}

pub(crate) fn only_selection(workspace: &Path) -> CwrResult<()> {
    let composition = match only_composition(workspace, "--only-runtime-selection") {
        Ok(composition) => composition,
        Err(error) => {
            if error.is::<cwr::ConfigurationError>() {
                serde_json::to_writer(
                    io::stdout(),
                    &serde_json::json!({
                        "error": { "code": "invalid_workspace", "message": error.to_string() }
                    }),
                )?;
                println!();
            }
            return Err(error);
        }
    };
    let selected = &composition.runtimes[0];
    serde_json::to_writer(
        io::stdout(),
        &OnlyRuntimeSelection {
            runtime_ref: selected.runtime_ref.clone(),
            working_directory: selected.working_directory.clone(),
            name: selected.name.clone(),
            runtime_directory: selected.runtime_directory()?,
        },
    )?;
    println!();
    Ok(())
}

fn only_composition(workspace: &Path, operation: &str) -> CwrResult<LocalWorkspaceComposition> {
    let composition = LocalWorkspaceComposition::load(workspace)?;
    let [selected] = composition.runtimes.as_slice() else {
        return Err(cwr::ConfigurationError(format!(
            "{operation} requires a workspace with exactly one configured runtime"
        ))
        .into());
    };
    selected.prepare_resident()?;
    Ok(composition)
}

pub(crate) fn control(runtime_directory: &Path, args: Vec<OsString>) -> CwrResult<()> {
    let [operation, rest @ ..] = args.as_slice() else {
        return Err(usage().into());
    };
    if operation == "--stop-resident" && rest.is_empty() {
        return cwr::v2026_09::local::stop_local_resident(
            &runtime_directory.join("resident-runtime.pid"),
            &runtime_directory.join("host.sock"),
        );
    }
    if operation == "--exit-resident-when-idle" && rest.is_empty() {
        return crate::retire_client::run_socket(&runtime_directory.join("runtime.sock"));
    }
    if operation == "--ensure-resident-environment" && rest.is_empty() {
        return crate::resident_environment::ensure(
            runtime_directory,
            &runtime_directory.join("runtime.sock"),
            &runtime_directory.join("host.sock"),
        );
    }
    if operation == "--resident-autostart-state" && rest.is_empty() {
        let paused = cwr::v2026_09::local::local_resident_autostart_paused(runtime_directory)?;
        println!("{}", serde_json::json!({"paused": paused}));
        return Ok(());
    }
    if operation == "--set-resident-autostart-paused" {
        let [value] = rest else {
            return Err(usage().into());
        };
        let paused = match value.to_str() {
            Some("true") => true,
            Some("false") => false,
            _ => return Err(usage().into()),
        };
        cwr::v2026_09::local::set_local_resident_autostart_paused(runtime_directory, paused)?;
        println!(
            "{}",
            serde_json::json!({"paused": cwr::v2026_09::local::local_resident_autostart_paused(runtime_directory)?})
        );
        return Ok(());
    }
    if operation == "--arm-retirement" {
        let [receipt] = rest else {
            return Err(usage().into());
        };
        let expected: cwr::v2026_09::local::LocalResidentRetirement =
            serde_json::from_str(receipt.to_str().ok_or_else(usage)?)?;
        cwr::v2026_09::local::arm_existing_local_resident_retirement(runtime_directory, &expected)?;
        println!("{}", serde_json::json!({"armed": true}));
        return Ok(());
    }
    if operation == "--retire-resident" || operation == "--retirement-result" {
        let [receipt] = rest else {
            return Err(usage().into());
        };
        let expected: cwr::v2026_09::local::LocalResidentRetirement =
            serde_json::from_str(receipt.to_str().ok_or_else(usage)?)?;
        if operation == "--retire-resident" {
            cwr::v2026_09::local::retire_existing_local_resident(runtime_directory, &expected)?;
        }
        let completed = cwr::v2026_09::local::completed_local_resident_retirement(
            runtime_directory,
            &expected,
        )?;
        println!("{}", serde_json::json!({"completed": completed}));
        return Ok(());
    }
    if operation == "--observe-resident" {
        let [expected] = rest else {
            return Err(usage().into());
        };
        return print_observation(runtime_directory, expected.to_str().ok_or_else(usage)?);
    }
    if operation == "--drain-resident" {
        let [operation] = rest else {
            return Err(usage().into());
        };
        return crate::drain_client::run_for_socket(
            runtime_directory.join("runtime.sock"),
            runtime_directory.to_owned(),
            operation.to_str().ok_or_else(usage)?,
        );
    }
    Err(usage().into())
}

fn runtime_config(prepared: PreparedConfiguredRuntime) -> CwrResult<RuntimeConfig> {
    let selected = prepared.selected();
    let runtime_directory = selected.runtime_directory()?;
    Ok(RuntimeConfig {
        workspace: selected.cwr_toml.clone(),
        state_directory: selected.state_directory.clone(),
        socket_path: runtime_directory.join("runtime.sock"),
        host_socket_path: runtime_directory.join("host.sock"),
        runtime_directory,
        workspace_id: None,
        runtime_root_ref: selected.runtime_ref.clone(),
        acting_actor_ref: Some(prepared.acting_actor_ref.clone()),
        config_directory: selected.working_directory.clone(),
        communication_adapters: Vec::new(),
        expected_communication_adapters: 0,
        max_concurrent_conversations: std::num::NonZeroUsize::new(16).unwrap(),
        configured_runtime: Some(prepared),
    })
}

fn run_client(
    prepared: PreparedConfiguredRuntime,
    ingress: &str,
    acting_actor_only: bool,
) -> CwrResult<()> {
    let catalog = prepared.catalog()?;
    let workspace = HostWorkspace {
        warnings: prepared.warnings().to_vec(),
        workspace_ref: prepared.selected().runtime_ref.clone(),
        runtime_ref: prepared.selected().runtime_ref.clone(),
        actors: if acting_actor_only {
            vec![project_host_actor(&catalog.acting_actor)]
        } else {
            catalog
                .known_actors
                .iter()
                .map(project_host_actor)
                .collect()
        },
    };
    let mut input = BufReader::new(io::stdin());
    let mut output = io::stdout();
    loop {
        let Some(line) = crate::jsonl::read_delimited_line(&mut input)? else {
            return Ok(());
        };
        let command: HostCommand = serde_json::from_str(&line)?;
        let request_id = command.request_id().to_owned();
        let result = command.validate().and_then(|()| match &command {
            HostCommand::InspectWorkspace { .. } => Ok(false),
            HostCommand::SelectActor { actor_ref, .. }
                if actor_ref == &prepared.acting_actor_ref =>
            {
                Ok(true)
            }
            HostCommand::SelectActor { .. } => Err(io::Error::other(
                "configured runtime requires its configured acting coworker",
            )
            .into()),
            _ => Err(io::Error::other(
                "select an acting actor before sending conversation commands",
            )
            .into()),
        });
        match result {
            Ok(false) => crate::resident_client::write_event(
                &mut output,
                &HostEvent::WorkspaceInspected {
                    request_id,
                    workspace: workspace.clone(),
                    default_actor_ref: Some(prepared.acting_actor_ref.clone()),
                },
            )?,
            Ok(true) => {
                let connection = connect(&prepared, ingress, false);
                match connection {
                    Ok(mut stream) => {
                        serde_json::to_writer(&mut stream, &command)?;
                        stream.write_all(b"\n")?;
                        stream.flush()?;
                        return crate::resident_client::proxy_streams(stream, input, output);
                    }
                    Err(error) => crate::resident_client::write_event(
                        &mut output,
                        &HostEvent::CommandFailed {
                            request_id,
                            error: error.to_string(),
                        },
                    )?,
                }
            }
            Err(error) => crate::resident_client::write_event(
                &mut output,
                &HostEvent::CommandFailed {
                    request_id,
                    error: error.to_string(),
                },
            )?,
        }
    }
}

fn connect(
    prepared: &PreparedConfiguredRuntime,
    ingress: &str,
    ensure_only: bool,
) -> CwrResult<UnixStream> {
    use cwr::v2026_09::local::connect_local_resident;
    prepared.create_resident_directories()?;
    let runtime_directory = prepared.selected().runtime_directory()?;
    let socket = runtime_directory.join("host.sock");
    let reconnect_generation = std::env::var_os("CWR_RECONNECT_RUNTIME_GENERATION");
    let _generation_fence = reconnect_generation
        .is_none()
        .then(|| {
            cwr::v2026_09::local::prepare_dev_runtime_generation_for_resident(
                &runtime_directory,
                &std::env::current_exe()?,
            )
        })
        .transpose()?
        .flatten();
    let mut reconnect_lock = None;
    let mut stream = if let Some(generation) = reconnect_generation {
        let (stream, lock) = cwr::v2026_09::local::reconnect_dev_runtime_generation(
            &runtime_directory,
            &generation.to_string_lossy(),
        )?;
        reconnect_lock = Some(lock);
        stream
    } else {
        match connect_local_resident(&socket) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                prepared.create_resident_directories()?;
                // Serialize competing starters, then recheck under the lock. The runtime
                // still holds the database lease for its entire execution lifetime.
                let lock = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .open(
                        prepared
                            .selected()
                            .runtime_directory()?
                            .join("resident-start.lock"),
                    )?;
                lock.lock()?;
                match connect_local_resident(&socket) {
                    Ok(stream) => stream,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                        ) =>
                    {
                        start(prepared, &socket)?
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        }
    };
    let response = handshake(&mut stream, prepared, ingress, ensure_only)?;
    let response = if ensure_only && reconnect_lock.is_none() && unsupported_ensure_probe(&response)
    {
        drop(stream);
        eprintln!("Replacing an older configured runtime; active tasks will drain before restart.");
        cwr::v2026_09::local::replace_incompatible_local_resident(&runtime_directory)?;
        stream = start(prepared, &socket)?;
        handshake(&mut stream, prepared, ingress, ensure_only)?
    } else {
        response
    };
    if response["kind"] != "configured_loop_bound" {
        return Err(io::Error::other(
            response["error"]
                .as_str()
                .unwrap_or("configured runtime handshake failed"),
        )
        .into());
    }
    if !ensure_only {
        stream.set_read_timeout(None)?;
        stream.set_write_timeout(None)?;
    }
    drop(reconnect_lock);
    Ok(stream)
}

fn unsupported_ensure_probe(response: &serde_json::Value) -> bool {
    response["kind"] == "configured_loop_rejected"
        && response["error"]
            .as_str()
            .is_some_and(|error| error.starts_with("unknown field `ensure_only`,"))
}

fn handshake(
    stream: &mut UnixStream,
    prepared: &PreparedConfiguredRuntime,
    ingress: &str,
    ensure_only: bool,
) -> CwrResult<serde_json::Value> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
    serde_json::to_writer(
        &mut *stream,
        &Handshake {
            version: 1,
            selection: SessionBinding::new(prepared),
            ingress: ingress.into(),
            ensure_only,
        },
    )?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut line = String::new();
    BufReader::new(&*stream).read_line(&mut line)?;
    let response: serde_json::Value = serde_json::from_str(&line)?;
    Ok(response)
}

#[path = "configured_runtime_start.rs"]
mod launch;
use launch::start;
#[cfg(test)]
use launch::wait_for_started_resident;

pub(crate) fn bind(
    stream: &mut UnixStream,
    expected: &SessionBinding,
) -> CwrResult<Option<String>> {
    use std::io::Read;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    let outcome = (|| {
        let mut line = Vec::new();
        loop {
            let mut byte = [0];
            stream.read_exact(&mut byte)?;
            if byte[0] == b'\n' {
                break;
            }
            if line.len() >= 16 * 1024 {
                return Err(io::Error::other("configured runtime handshake too large").into());
            }
            line.push(byte[0]);
        }
        let handshake: Handshake = serde_json::from_slice(&line)?;
        if handshake.version != 1 || &handshake.selection != expected {
            return Err(io::Error::other("configured runtime selection or executable configuration differs from the active resident; its work continues unchanged").into());
        }
        let ingress = cwr::CwrRef::parse(&handshake.ingress)?;
        if handshake.ingress.starts_with("~@") || ingress.fragment().is_some() {
            return Err(
                io::Error::other("ingress must be an absolute ref without a fragment").into(),
            );
        }
        Ok((!handshake.ensure_only).then_some(handshake.ingress))
    })();
    let response = match &outcome {
        Ok(_) => serde_json::json!({"kind": "configured_loop_bound"}),
        Err(error) => {
            serde_json::json!({"kind": "configured_loop_rejected", "error": format!("{error}")})
        }
    };
    serde_json::to_writer(&mut *stream, &response)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    stream.set_read_timeout(None)?;
    outcome
}

#[cfg(test)]
#[path = "configured_runtime_handshake_tests.rs"]
mod handshake_tests;

fn print_observation(directory: &Path, expected_runtime_ref: &str) -> CwrResult<()> {
    let observation =
        cwr::v2026_09::local::observe_existing_local_resident(directory, expected_runtime_ref)?;
    println!("{}", serde_json::to_string(&observation)?);
    Ok(())
}
