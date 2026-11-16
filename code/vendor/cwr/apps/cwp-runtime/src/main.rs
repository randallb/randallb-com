mod bridge;
mod build_identity;
mod cli;
mod cloud_execution;
mod config;
mod configured_runtime;
mod drain_client;
mod graphql;
mod host_server;
mod host_session;
mod idle;
mod jsonl;
mod live_status;
mod resident_client;
mod resident_environment;
mod retire_client;
mod server;

use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use cwr::v2026_09::local::{LocalHostIdentity, LocalRuntime, LocalRuntimeOptions};
use cwr::v2026_09::startup_profile::{time as time_startup, StartupPhase};

use config::RuntimeConfig;

const RUNTIME_INGRESS_REF: &str = "cwp://org.coworkerprotocol.local/integrations/runtime-service";
const HOST_STDIO_INGRESS_REF: &str = "cwp://org.coworkerprotocol.local/integrations/host-stdio";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeMode {
    Service,
    HostStdio,
}

fn main() -> cwr::CwrResult<()> {
    // A zero-length marker aligns the resident's phase log with launcher time.
    drop(StartupPhase::new("resident.process_entry"));
    // Launchers provide the state path before configuration parsing can fail.
    let boot_diagnostics = env::var_os("CWR_RESIDENT_DIAGNOSTIC_STATE_DIRECTORY")
        .map(PathBuf::from)
        .map(|path| cwr::capture_resident_stderr(&path))
        .transpose()?;
    let result = cli::run();
    if boot_diagnostics.is_some() {
        if let Err(error) = &result {
            eprintln!("cwr: resident failed: {error}");
        }
    }
    result
}

fn run_runtime(
    config: &RuntimeConfig,
    acting_actor_ref: &str,
    mode: RuntimeMode,
    selected_actor: Option<host_session::SelectedActor>,
    host_input: Option<std::sync::mpsc::Receiver<bridge::HostInput>>,
    host_output: Option<bridge::output::SharedHostWriter>,
) -> cwr::CwrResult<()> {
    let diagnostics = if mode == RuntimeMode::Service
        && env::var_os("CWR_RESIDENT_DIAGNOSTIC_STATE_DIRECTORY").is_none()
    {
        cwr::capture_resident_stderr(&config.state_directory).ok()
    } else {
        None
    };
    let result = run_runtime_inner(
        config,
        acting_actor_ref,
        mode,
        selected_actor,
        host_input,
        host_output,
    );
    if diagnostics.is_some() {
        if let Err(error) = &result {
            eprintln!("cwr: resident failed: {error}");
        }
    }
    result
}

fn run_runtime_inner(
    config: &RuntimeConfig,
    acting_actor_ref: &str,
    mode: RuntimeMode,
    selected_actor: Option<host_session::SelectedActor>,
    host_input: Option<std::sync::mpsc::Receiver<bridge::HostInput>>,
    host_output: Option<bridge::output::SharedHostWriter>,
) -> cwr::CwrResult<()> {
    let startup_profile = StartupPhase::new("resident.startup.total");
    let workspace_binding = config.workspace_binding()?;
    let host_identity = LocalHostIdentity::new(acting_actor_ref, runtime_ingress_ref(mode));
    let options = LocalRuntimeOptions {
        max_concurrent_conversations: config.max_concurrent_conversations,
        ..LocalRuntimeOptions::configured()?
    };
    let runtime_result = time_startup("resident.local_runtime_open", || {
        if let Some(prepared) = &config.configured_runtime {
            prepared.open(runtime_ingress_ref(mode), options)
        } else {
            match workspace_binding.clone() {
                None => LocalRuntime::open_with_state_path_and_runtime_ref_and_options(
                    &config.workspace,
                    config.state_directory.join("cwpdb.sqlite3"),
                    &config.runtime_root_ref,
                    host_identity.clone(),
                    options,
                ),
                Some(binding) => LocalRuntime::open_bound_with_options(
                    &config.workspace,
                    config.state_directory.join("cwpdb.sqlite3"),
                    &config.runtime_root_ref,
                    host_identity,
                    options,
                    binding,
                ),
            }
        }
    });
    let runtime = match runtime_result {
        Ok(runtime) => runtime,
        Err(error) => return reject_startup(selected_actor.as_ref(), host_output.as_ref(), error),
    };
    let _cloud_execution =
        cloud_execution::start_from_environment(runtime.clone(), &config.state_directory)?;
    let selected_catalog = if selected_actor.is_some() {
        match runtime.catalog() {
            Ok(catalog) => Some(catalog),
            Err(error) => {
                return reject_startup(selected_actor.as_ref(), host_output.as_ref(), error)
            }
        }
    } else {
        None
    };
    let graphql_runtime = runtime.clone();
    let mut server = match time_startup("resident.server_start", || {
        server::start(
            &config.socket_path,
            config.runtime_root_ref.clone(),
            runtime,
            config.state_directory.join("conversation-attachments.json"),
            config.expected_communication_adapters,
        )
    }) {
        Ok(server) => server,
        Err(error) => return reject_startup(selected_actor.as_ref(), host_output.as_ref(), error),
    };
    if mode == RuntimeMode::Service && config.configured_runtime.is_some() {
        server.enable_configured_retirement()?;
    }
    let graphql = time_startup("resident.graphql_start", || {
        graphql::start(
            graphql_runtime,
            config.runtime_root_ref.clone(),
            &config.runtime_directory,
            server.activity.clone(),
            server.graphql_admission_publisher(),
            server.graphql_changes(),
        )
    })?;
    time_startup("resident.server_accept", || server.accept_connections())?;
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    let _resident_pid = (mode == RuntimeMode::Service)
        .then(|| ResidentPid::publish(&config.runtime_directory))
        .transpose()?;
    let _resident_environment = (mode == RuntimeMode::Service)
        .then(|| {
            resident_environment::ResidentEnvironmentMarker::publish(&config.runtime_directory)
        })
        .transpose()?;
    let _host_server = if mode == RuntimeMode::Service {
        match time_startup("resident.host_socket_bind", || {
            host_server::start(
                &config.host_socket_path,
                &config.socket_path,
                &config.runtime_root_ref,
                &config.state_directory,
                match &config.configured_runtime {
                    Some(prepared) => host_server::SessionSelection::Configured(
                        configured_runtime::SessionBinding::new(prepared),
                    ),
                    None => workspace_binding.map_or(
                        host_server::SessionSelection::Unbound,
                        host_server::SessionSelection::Workspace,
                    ),
                },
                Arc::clone(&shutdown_requested),
                server.activity.clone(),
            )
        }) {
            Ok(server) => Some(server),
            Err(error) => {
                return reject_startup(selected_actor.as_ref(), host_output.as_ref(), error)
            }
        }
    } else {
        None
    };
    drop(startup_profile);
    if let Some(environment) = &_resident_environment {
        if let Err(error) = environment.validate_requirement(&config.runtime_directory) {
            return reject_startup(selected_actor.as_ref(), host_output.as_ref(), error);
        }
    }
    let mut adapters = match spawn_communication_adapters(config) {
        Ok(adapters) => adapters,
        Err(error) => return reject_startup(selected_actor.as_ref(), host_output.as_ref(), error),
    };
    if mode == RuntimeMode::Service {
        signal_hook::flag::register(
            signal_hook::consts::SIGTERM,
            Arc::clone(&shutdown_requested),
        )?;
        signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown_requested))?;
        server.recover_pending()?;
    }
    let auto_exit = mode == RuntimeMode::Service && adapters.is_empty();
    let outcome = selected_actor
        .map_or(Ok(()), |selected_actor| {
            host_session::announce_actor(
                selected_actor,
                selected_catalog
                    .as_ref()
                    .expect("a selected actor has an authoritative runtime catalog"),
                host_output
                    .as_ref()
                    .expect("a selected host actor has a bounded output writer"),
            )
        })
        .and_then(|()| {
            supervise_runtime(
                config,
                mode,
                acting_actor_ref,
                &mut adapters,
                host_input,
                host_output,
                || {
                    if shutdown_requested.load(Ordering::SeqCst) {
                        return Ok(true);
                    }
                    server.should_exit(auto_exit)
                },
            )
        });
    server.stop_accepting();
    let retirement = server.freeze_retirement_control();
    drop(graphql);
    let shutdown = server.shutdown();
    let adapter_shutdown = stop_communication_adapters(&mut adapters);
    finish_retirement(
        &config.runtime_directory,
        outcome,
        shutdown,
        adapter_shutdown,
        retirement,
    )
}

fn finish_retirement(
    directory: &std::path::Path,
    outcome: cwr::CwrResult<()>,
    shutdown: cwr::CwrResult<Vec<String>>,
    adapter_shutdown: cwr::CwrResult<()>,
    retirement: cwr::CwrResult<Option<cwr::v2026_09::local::LocalResidentRetirement>>,
) -> cwr::CwrResult<()> {
    finish_runtime(outcome, shutdown)?;
    adapter_shutdown?;
    if let Some(receipt) = retirement? {
        cwr::v2026_09::local::publish_completed_local_resident_retirement(directory, &receipt)?;
    }
    Ok(())
}

struct ResidentPid {
    path: PathBuf,
    pid: u32,
}

impl ResidentPid {
    fn publish(state_directory: &std::path::Path) -> cwr::CwrResult<Self> {
        let path = state_directory.join("resident-runtime.pid");
        let pid = std::process::id();
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        writeln!(file, "{pid}")?;
        file.sync_all()?;
        Ok(Self { path, pid })
    }
}

impl Drop for ResidentPid {
    fn drop(&mut self) {
        let owned = fs::read_to_string(&self.path)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            == Some(self.pid);
        if owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn finish_runtime(
    outcome: cwr::CwrResult<()>,
    shutdown: cwr::CwrResult<Vec<String>>,
) -> cwr::CwrResult<()> {
    match (outcome, shutdown) {
        (Ok(()), Ok(_)) => Ok(()),
        (Err(error), Ok(_)) | (Ok(()), Err(error)) => Err(error),
        (Err(runtime_error), Err(shutdown_error)) => Err(std::io::Error::other(format!(
            "runtime failed: {runtime_error}; managed shutdown failed: {shutdown_error}"
        ))
        .into()),
    }
}

fn reject_startup<T>(
    selected_actor: Option<&host_session::SelectedActor>,
    host_output: Option<&bridge::output::SharedHostWriter>,
    error: Box<dyn std::error::Error + Send + Sync>,
) -> cwr::CwrResult<T> {
    if let Some(selected_actor) = selected_actor {
        host_session::reject_actor(
            &selected_actor.request_id,
            &error.to_string(),
            host_output.expect("a selected host actor has a bounded output writer"),
        )?;
    }
    Err(error)
}

fn supervise_runtime(
    config: &RuntimeConfig,
    mode: RuntimeMode,
    acting_actor_ref: &str,
    adapters: &mut [std::process::Child],
    host_input: Option<std::sync::mpsc::Receiver<bridge::HostInput>>,
    host_output: Option<bridge::output::SharedHostWriter>,
    should_stop: impl Fn() -> cwr::CwrResult<bool>,
) -> cwr::CwrResult<()> {
    let bridge = bridge_ingress_ref(mode)
        .map(|ingress_ref| {
            let (sender, receiver) = std::sync::mpsc::channel();
            let socket_path = config.socket_path.clone();
            let runtime_root_ref = config.runtime_root_ref.clone();
            let bound_actor_ref =
                (mode != RuntimeMode::Service).then(|| acting_actor_ref.to_owned());
            let performance_database_path = (ingress_ref == bridge::WORKLOOP_DESKTOP_INGRESS_REF)
                .then(|| {
                    config
                        .state_directory
                        .join(cwr::LOCAL_METRICS_DATABASE_FILENAME)
                });
            thread::Builder::new()
                .name("cwp-runtime-host-bridge".into())
                .spawn(move || {
                    let outcome = match (host_input, host_output) {
                        (None, None) => bridge::run(
                            &socket_path,
                            &runtime_root_ref,
                            ingress_ref,
                            bound_actor_ref.as_deref(),
                            performance_database_path.clone(),
                        ),
                        (Some(input), Some(output)) => bridge::run_with_input(
                            &socket_path,
                            &runtime_root_ref,
                            ingress_ref,
                            bound_actor_ref.as_deref(),
                            performance_database_path,
                            input,
                            output,
                        ),
                        _ => Err(std::io::Error::other(
                            "host input and output must be provided together",
                        )
                        .into()),
                    };
                    let _ = sender.send(outcome);
                })?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(receiver)
        })
        .transpose()?;
    loop {
        cwr::v2026_09::local::reject_retired_shared_folder(&config.workspace)?;
        if should_stop()? {
            return Ok(());
        }
        for (index, adapter) in adapters.iter_mut().enumerate() {
            if let Some(status) = adapter.try_wait()? {
                return Err(std::io::Error::other(format!(
                    "communication adapter {} exited unexpectedly ({status})",
                    index + 1
                ))
                .into());
            }
        }
        if let Some(bridge) = &bridge {
            match bridge.try_recv() {
                Ok(outcome) => return outcome,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err(std::io::Error::other("runtime host bridge exited").into())
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn runtime_ingress_ref(mode: RuntimeMode) -> &'static str {
    match mode {
        RuntimeMode::Service => RUNTIME_INGRESS_REF,
        RuntimeMode::HostStdio => HOST_STDIO_INGRESS_REF,
    }
}

fn bridge_ingress_ref(mode: RuntimeMode) -> Option<&'static str> {
    match mode {
        RuntimeMode::Service => None,
        RuntimeMode::HostStdio => Some(HOST_STDIO_INGRESS_REF),
    }
}

fn spawn_communication_adapters(
    config: &RuntimeConfig,
) -> cwr::CwrResult<Vec<std::process::Child>> {
    spawn_communication_adapters_with(config, |command| command.spawn().map_err(Into::into))
}

fn spawn_communication_adapters_with(
    config: &RuntimeConfig,
    mut spawn: impl FnMut(&mut Command) -> cwr::CwrResult<std::process::Child>,
) -> cwr::CwrResult<Vec<std::process::Child>> {
    let mut children = Vec::new();
    for adapter in &config.communication_adapters {
        let outcome = (|| {
            let command = if adapter.command.components().count() == 1 {
                env::current_exe()?.with_file_name(&adapter.command)
            } else if adapter.command.is_absolute() {
                adapter.command.clone()
            } else {
                config.config_directory.join(&adapter.command)
            };
            let socket = config.socket_path.to_string_lossy();
            let args = adapter
                .args
                .iter()
                .map(|argument| argument.replace("{runtime_socket}", &socket))
                .collect::<Vec<_>>();
            let mut command = Command::new(command);
            command
                .args(args)
                .env_remove("CWR_RESIDENT_DIAGNOSTIC_STATE_DIRECTORY")
                .current_dir(&config.config_directory)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::inherit());
            spawn(&mut command)
        })();
        match outcome {
            Ok(child) => children.push(child),
            Err(error) => {
                if let Err(cleanup) = stop_communication_adapters(&mut children) {
                    eprintln!("communication adapter cleanup failed: {cleanup}");
                }
                return Err(error);
            }
        }
    }
    Ok(children)
}

fn stop_communication_adapters(adapters: &mut [std::process::Child]) -> cwr::CwrResult<()> {
    let mut failures = Vec::new();
    for adapter in adapters {
        match adapter.try_wait() {
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(error) => {
                failures.push(error.to_string());
                continue;
            }
        }
        if let Err(error) = adapter.kill() {
            failures.push(error.to_string());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match adapter.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Ok(None) => {
                    failures.push("communication adapter reap timed out".into());
                    break;
                }
                Err(error) => {
                    failures.push(error.to_string());
                    break;
                }
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "communication adapters did not shut down: {}",
            failures.join("; ")
        ))
        .into())
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/support/refs.rs"]
mod test_refs;
