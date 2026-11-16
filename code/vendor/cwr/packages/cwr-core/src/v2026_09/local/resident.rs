const WORKSPACE_HANDSHAKE_VERSION: u32 = 1;

#[derive(Serialize)]
struct WorkspaceHandshake<'a> {
    version: u32,
    workspace_id: &'a str,
    workspace_path: &'a Path,
    ingress_integration_ref: &'a str,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[expect(
    clippy::enum_variant_names,
    reason = "variant names preserve the resident workspace handshake wire protocol"
)]
enum WorkspaceHandshakeResponse {
    WorkspaceBound { version: u32 },
    WorkspaceMoved { version: u32 },
    WorkspaceRejected { version: u32, error: String },
}

enum WorkspaceBindOutcome {
    Bound,
    WorkspaceMoved,
}

include!("resident/dev_generation.rs");
include!("resident/restart.rs");
include!("resident/observation.rs");
include!("resident/retirement.rs");

pub fn connect_or_start_local_resident(
    workspace_paths: &LocalWorkspacePaths,
    acting_actor_ref: Option<&str>,
    executable: &Path,
    ingress_integration_ref: &str,
) -> CwrResult<Option<std::os::unix::net::UnixStream>> {
    connect_or_start_resident(
        workspace_paths,
        acting_actor_ref,
        executable,
        ingress_integration_ref,
        false,
    )
}

/// Start or attach to a resident that supports session-bound TUI GraphQL ingress.
/// Older residents drain and restart with the selected executable and retained state.
pub fn connect_or_start_local_graphql_resident(
    workspace_paths: &LocalWorkspacePaths,
    acting_actor_ref: Option<&str>,
    executable: &Path,
    ingress_integration_ref: &str,
) -> CwrResult<Option<std::os::unix::net::UnixStream>> {
    connect_or_start_resident(
        workspace_paths,
        acting_actor_ref,
        executable,
        ingress_integration_ref,
        true,
    )
}

fn connect_or_start_resident(
    workspace_paths: &LocalWorkspacePaths,
    acting_actor_ref: Option<&str>,
    executable: &Path,
    ingress_integration_ref: &str,
    require_tui_graphql: bool,
) -> CwrResult<Option<std::os::unix::net::UnixStream>> {
    reject_retired_shared_folder(&workspace_paths.cwr_toml)?;
    if let Some(generation) = std::env::var_os("CWR_RECONNECT_RUNTIME_GENERATION") {
        return reconnect_local_resident(
            workspace_paths,
            &generation.to_string_lossy(),
            ingress_integration_ref,
        )
        .map(Some);
    }
    let _generation_fence = prepare_dev_runtime_generation(workspace_paths, executable)?;
    let socket_path = workspace_paths.host_socket_path();
    let stream = match connect_local_resident(&socket_path) {
        Ok(stream) => Ok(Some(stream)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            let acting_actor_ref = resident_startup_actor(workspace_paths, acting_actor_ref)?;
            start_local_resident(workspace_paths, &socket_path, &acting_actor_ref, executable)
        }
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!(
                "could not connect to resident CWR runtime {}: {error}",
                socket_path.display()
            ),
        )
        .into()),
    }?;
    let Some(mut stream) = stream else {
        return Ok(None);
    };
    match bind_local_resident_workspace_inner(
        &mut stream,
        &workspace_paths.identity.workspace_id,
        &workspace_paths.workspace_directory,
        ingress_integration_ref,
    )? {
        WorkspaceBindOutcome::Bound
            if require_tui_graphql
                && !resident_supports_tui_graphql(&stream, &workspace_paths.runtime_directory)? =>
        {
            drop(stream);
            eprintln!("Replacing an older runtime for TUI GraphQL support; active tasks will drain.");
            replace_incompatible_local_resident(&workspace_paths.runtime_directory)?;
            let acting_actor_ref = resident_startup_actor(workspace_paths, acting_actor_ref)?;
            let Some(mut stream) =
                start_local_resident(workspace_paths, &socket_path, &acting_actor_ref, executable)?
            else {
                return Ok(None);
            };
            bind_local_resident_workspace(
                &mut stream,
                &workspace_paths.identity.workspace_id,
                &workspace_paths.workspace_directory,
                ingress_integration_ref,
            )?;
            if !resident_supports_tui_graphql(&stream, &workspace_paths.runtime_directory)? {
                return invalid("replacement resident does not support TUI GraphQL ingress");
            }
            Ok(Some(stream))
        }
        WorkspaceBindOutcome::Bound => Ok(Some(stream)),
        WorkspaceBindOutcome::WorkspaceMoved => {
            drop(stream);
            wait_for_local_resident_shutdown(&socket_path)?;
            let acting_actor_ref = resident_startup_actor(workspace_paths, acting_actor_ref)?;
            let Some(mut stream) =
                start_local_resident(workspace_paths, &socket_path, &acting_actor_ref, executable)?
            else {
                return Ok(None);
            };
            match bind_local_resident_workspace_inner(
                &mut stream,
                &workspace_paths.identity.workspace_id,
                &workspace_paths.workspace_directory,
                ingress_integration_ref,
            )? {
                WorkspaceBindOutcome::Bound => Ok(Some(stream)),
                WorkspaceBindOutcome::WorkspaceMoved => {
                    invalid("replacement resident still owns the workspace's previous path")
                }
            }
        }
    }
}

fn resident_startup_actor(
    workspace_paths: &LocalWorkspacePaths,
    acting_actor_ref: Option<&str>,
) -> CwrResult<String> {
    acting_actor_ref.map(str::to_owned).map_or_else(
        || load_default_local_acting_actor_ref(&workspace_paths.cwr_toml),
        Ok,
    )
}

pub fn bind_local_resident_workspace(
    stream: &mut std::os::unix::net::UnixStream,
    workspace_id: &str,
    workspace_path: &Path,
    ingress_integration_ref: &str,
) -> CwrResult<()> {
    match bind_local_resident_workspace_inner(
        stream,
        workspace_id,
        workspace_path,
        ingress_integration_ref,
    )? {
        WorkspaceBindOutcome::Bound => Ok(()),
        WorkspaceBindOutcome::WorkspaceMoved => {
            invalid("resident runtime is restarting because the workspace moved; retry the command")
        }
    }
}

fn bind_local_resident_workspace_inner(
    stream: &mut std::os::unix::net::UnixStream,
    workspace_id: &str,
    workspace_path: &Path,
    ingress_integration_ref: &str,
) -> CwrResult<WorkspaceBindOutcome> {
    use std::io::{BufRead as _, BufReader, Write as _};

    serde_json::to_writer(
        &mut *stream,
        &WorkspaceHandshake {
            version: WORKSPACE_HANDSHAKE_VERSION,
            workspace_id,
            workspace_path,
            ingress_integration_ref,
        },
    )?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut response = String::new();
    if BufReader::new(stream.try_clone()?).read_line(&mut response)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "resident runtime closed before binding the workspace",
        )
        .into());
    }
    match serde_json::from_str(&response)? {
        WorkspaceHandshakeResponse::WorkspaceBound { version } => {
            if version == WORKSPACE_HANDSHAKE_VERSION {
                Ok(WorkspaceBindOutcome::Bound)
            } else {
                invalid(format!(
                    "unsupported resident workspace response version {version}"
                ))
            }
        }
        WorkspaceHandshakeResponse::WorkspaceMoved { version } => {
            if version == WORKSPACE_HANDSHAKE_VERSION {
                Ok(WorkspaceBindOutcome::WorkspaceMoved)
            } else {
                invalid(format!(
                    "unsupported resident workspace response version {version}"
                ))
            }
        }
        WorkspaceHandshakeResponse::WorkspaceRejected { version, error } => {
            if version == WORKSPACE_HANDSHAKE_VERSION {
                invalid(error)
            } else {
                invalid(format!(
                    "unsupported resident workspace response version {version}"
                ))
            }
        }
    }
}

fn wait_for_local_resident_shutdown(path: &Path) -> CwrResult<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match connect_local_resident(path) {
            Ok(stream) if Instant::now() < deadline => {
                drop(stream);
                thread::sleep(Duration::from_millis(20));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(())
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "resident CWR runtime did not stop after its workspace moved",
                )
                .into())
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub fn connect_local_resident(path: &Path) -> io::Result<std::os::unix::net::UnixStream> {
    let stream = std::os::unix::net::UnixStream::connect(path)?;
    verify_local_resident_peer(&stream)?;
    Ok(stream)
}

fn start_local_resident(
    workspace_paths: &LocalWorkspacePaths,
    socket_path: &Path,
    acting_actor_ref: &str,
    executable: &Path,
) -> CwrResult<Option<std::os::unix::net::UnixStream>> {
    use std::{
        os::unix::process::CommandExt as _,
        process::Stdio,
    };

    if workspace_paths
        .runtime_directory
        .join("resident-stopped")
        .exists()
    {
        return invalid(
            "Background work is stopped. Use Debug → Restart Background Runtime to start it.",
        );
    }
    if !executable.is_file() {
        return Ok(None);
    }
    let state_directory = &workspace_paths.state_directory;
    let config_path = state_directory.join("resident-runtime.toml");
    let temporary_config =
        state_directory.join(format!(".resident-runtime-{}.toml", std::process::id()));
    let config = resident_runtime_config(workspace_paths, acting_actor_ref)?;
    fs::write(&temporary_config, config)?;
    fs::set_permissions(&temporary_config, fs::Permissions::from_mode(0o600))?;
    fs::rename(&temporary_config, &config_path)?;

    let log_path = state_directory.join("diagnostics/wlpdb.sqlite3");
    let mut command = std::process::Command::new(executable);
    command
        .arg(&config_path)
        .current_dir(&workspace_paths.workspace_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("CWR_RESIDENT_DIAGNOSTIC_STATE_DIRECTORY", state_directory);
    // SAFETY: setsid runs in the child after fork and before exec, touches no
    // shared Rust state, and detaches the independently owned resident runtime.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    // Loading retained history can take arbitrarily long. Only an exited
    // process or a permanent connection failure means startup failed.
    loop {
        match connect_local_resident(socket_path) {
            Ok(stream) => {
                if local_resident_peer_pid(&stream)? != i32::try_from(child.id())? {
                    return invalid("replacement resident socket belongs to a different process");
                }
                if child.try_wait()?.is_none() {
                    let _ = thread::Builder::new()
                        .name("cwr-resident-reaper".into())
                        .spawn(move || {
                            let _ = child.wait();
                        });
                }
                return Ok(Some(stream));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                if let Some(status) = child.try_wait()? {
                    return invalid(format!(
                        "replacement resident exited during startup ({status}); see {}",
                        log_path.display()
                    ));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "resident CWR runtime did not start; see {}: {error}",
                        log_path.display()
                    ),
                )
                .into());
            }
        }
    }
}

fn resident_runtime_config(
    workspace_paths: &LocalWorkspacePaths,
    acting_actor_ref: &str,
) -> CwrResult<String> {
    let workspace_config = workspace_paths
        .workspace_directory
        .join(".cwr/runtime.toml");
    let mut config = if workspace_config.is_file() {
        let source = fs::read_to_string(&workspace_config)?;
        let config = source.parse::<toml::Table>()?;
        if config.get("version").and_then(toml::Value::as_integer) != Some(1) {
            return invalid(format!(
                "{}: resident runtime configuration must use version 1",
                workspace_config.display()
            ));
        }
        config
    } else {
        toml::Table::new()
    };
    // A workspace is repository-controlled input. Starting a resident because a
    // user opened that workspace must not execute commands declared by it.
    config.remove("communication_adapters");
    config.remove("communication_adapter_directory");
    config.insert("version".into(), toml::Value::Integer(1));
    set_resident_config_path(&mut config, "workspace", &workspace_paths.cwr_toml);
    set_resident_config_path(
        &mut config,
        "state_directory",
        &workspace_paths.state_directory,
    );
    set_resident_config_path(
        &mut config,
        "runtime_directory",
        &workspace_paths.runtime_directory,
    );
    set_resident_config_path(
        &mut config,
        "socket_path",
        &workspace_paths.runtime_socket_path(),
    );
    set_resident_config_path(
        &mut config,
        "host_socket_path",
        &workspace_paths.host_socket_path(),
    );
    config.insert(
        "workspace_id".into(),
        toml::Value::String(workspace_paths.identity.workspace_id.clone()),
    );
    config.insert(
        "runtime_root_ref".into(),
        toml::Value::String(workspace_paths.runtime_root_ref.clone()),
    );
    config.insert(
        "acting_actor_ref".into(),
        toml::Value::String(acting_actor_ref.into()),
    );
    Ok(toml::to_string(&config)?)
}

fn set_resident_config_path(config: &mut toml::Table, key: &str, path: &Path) {
    config.insert(
        key.into(),
        toml::Value::String(path.to_string_lossy().into_owned()),
    );
}

#[cfg(target_vendor = "apple")]
fn verify_local_resident_peer(stream: &std::os::unix::net::UnixStream) -> io::Result<()> {
    use std::os::fd::AsRawFd as _;

    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: the descriptor is a live Unix socket and both credential pointers are writable.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    verify_local_resident_uid(uid)
}

#[cfg(target_os = "linux")]
fn verify_local_resident_peer(stream: &std::os::unix::net::UnixStream) -> io::Result<()> {
    use std::os::fd::AsRawFd as _;

    // SAFETY: ucred is a C POD type populated by getsockopt.
    let mut credentials = unsafe { std::mem::zeroed::<libc::ucred>() };
    let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: the descriptor is live and the credential buffers are valid for writes.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    verify_local_resident_uid(credentials.uid)
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
fn verify_local_resident_peer(_stream: &std::os::unix::net::UnixStream) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "resident peer credential inspection is unavailable on this platform",
    ))
}

fn verify_local_resident_uid(peer_uid: libc::uid_t) -> io::Result<()> {
    // SAFETY: geteuid reads process credentials and has no preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    if peer_uid == effective_uid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("resident runtime belongs to user {peer_uid}, not user {effective_uid}"),
        ))
    }
}

#[cfg(test)]
mod resident_tests {
    use super::*;

    #[test]
    fn slow_resident_is_not_killed_at_the_old_startup_deadline() {
        let temporary = tempfile::tempdir_in("/tmp").unwrap();
        let workspace = temporary.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("CWR.toml"), "version = '2026-09'\n").unwrap();
        let paths = LocalWorkspacePaths::resolve_with_base_directories(
            workspace.join("CWR.toml"),
            temporary.path().join("state"),
            temporary.path().join("runtime"),
        )
        .unwrap();
        let executable = temporary.path().join("slow-runtime");
        fs::write(&executable, "#!/bin/sh\nsleep 11\nexit 7\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let error = start_local_resident(
            &paths,
            &paths.host_socket_path(),
            "~@/actors/person",
            &executable,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("exited during startup"),
            "{error}"
        );
        assert!(error.to_string().contains('7'), "{error}");
    }

    #[test]
    fn generated_resident_config_does_not_execute_workspace_adapters() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        fs::create_dir_all(workspace.join(".cwr")).unwrap();
        fs::write(workspace.join("CWR.toml"), "version = '2026-09'\n").unwrap();
        fs::write(
            workspace.join(".cwr/runtime.toml"),
            "version = 1\nworkspace = '../../old.toml'\n[[communication_adapters]]\ncommand = './adapter'\nargs = ['--socket={runtime_socket}']\n",
        )
        .unwrap();
        let paths = LocalWorkspacePaths::resolve_with_base_directories(
            workspace.join("CWR.toml"),
            temporary.path().join("state"),
            temporary.path().join("runtime"),
        )
        .unwrap();

        let config: toml::Value = resident_runtime_config(&paths, "~@/actors/person")
            .unwrap()
            .parse()
            .unwrap();

        assert!(config.get("communication_adapters").is_none());
        assert!(config.get("communication_adapter_directory").is_none());
        assert_eq!(
            config["state_directory"].as_str(),
            Some(paths.state_directory.to_string_lossy().as_ref())
        );
        assert_eq!(
            config["acting_actor_ref"].as_str(),
            Some("~@/actors/person")
        );
    }
}
