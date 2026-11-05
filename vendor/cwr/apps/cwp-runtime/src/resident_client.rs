use std::{
    env,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    os::unix::process::CommandExt as _,
    path::Path,
    process::{Child, Stdio},
    thread,
    time::Duration,
};

use cwr::v2026_09::{
    host::{HostCommand, HostEvent},
    local::{
        bind_local_resident_workspace, connect_local_resident, connect_or_start_local_resident,
        inspect_local_runtime_workspace, load_local_catalog,
        load_optional_default_local_acting_actor_ref, LocalWorkspacePaths,
    },
};

use crate::config::RuntimeConfig;

pub(crate) fn run(config: &RuntimeConfig, ingress_ref: &str) -> cwr::CwrResult<()> {
    run_for_paths(config.workspace_paths()?, ingress_ref)
}

pub(crate) fn run_workspace(workspace: &std::path::Path, ingress_ref: &str) -> cwr::CwrResult<()> {
    run_for_paths(LocalWorkspacePaths::resolve(workspace)?, ingress_ref)
}

pub(crate) fn run_configured(
    config_path: &Path,
    config: &RuntimeConfig,
    ingress_ref: &str,
) -> cwr::CwrResult<()> {
    cwr::v2026_09::local::reject_retired_shared_folder(&config.workspace)?;
    let mut stream = connect_or_start_configured_resident(config_path, config)?;
    if let Some(binding) = config.workspace_binding()? {
        bind_local_resident_workspace(
            &mut stream,
            &binding.workspace_id,
            &binding.workspace_directory,
            ingress_ref,
        )?;
    }
    proxy_streams(stream, io::stdin(), io::stdout())
}

fn connect_or_start_configured_resident(
    config_path: &Path,
    config: &RuntimeConfig,
) -> cwr::CwrResult<UnixStream> {
    match connect_local_resident(&config.host_socket_path) {
        Ok(stream) => Ok(stream),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            start_configured_resident(config_path, config)
        }
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!(
                "could not connect to configured CWR runtime {}: {error}",
                config.host_socket_path.display()
            ),
        )
        .into()),
    }
}

fn start_configured_resident(
    config_path: &Path,
    config: &RuntimeConfig,
) -> cwr::CwrResult<UnixStream> {
    if config.runtime_directory.join("resident-stopped").exists() {
        return Err(io::Error::other(
            "Background work is stopped. Use Debug → Restart Background Runtime to start it.",
        )
        .into());
    }
    let executable = env::current_exe()?;
    let log_path = config.state_directory.join("diagnostics/wlpdb.sqlite3");
    let mut command = std::process::Command::new(executable);
    command
        .arg(config_path)
        .current_dir(&config.config_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(
            "CWR_RESIDENT_DIAGNOSTIC_STATE_DIRECTORY",
            &config.state_directory,
        );
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
    wait_for_configured_resident(command.spawn()?, &config.host_socket_path, &log_path)
}

fn wait_for_configured_resident(
    mut child: Child,
    socket: &Path,
    log_path: &Path,
) -> cwr::CwrResult<UnixStream> {
    // Loading retained state can take longer than a fixed startup window.
    // Only an exited child or a permanent socket error ends the wait.
    loop {
        match connect_local_resident(socket) {
            Ok(stream) => {
                // A competing starter may own this socket after our child exits.
                reap_configured_resident(child);
                return Ok(stream);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                if let Some(status) = child.try_wait()? {
                    return Err(io::Error::other(format!(
                        "configured CWR runtime exited during startup ({status}); see {}",
                        log_path.display()
                    ))
                    .into());
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "configured CWR runtime did not start; see {}: {error}",
                        log_path.display()
                    ),
                )
                .into());
            }
        }
    }
}

fn reap_configured_resident(mut child: Child) {
    let _ = thread::Builder::new()
        .name("cwr-configured-resident-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        });
}

#[cfg(test)]
#[path = "resident_client_tests.rs"]
mod configured_launch_tests;

fn run_for_paths(workspace_paths: LocalWorkspacePaths, ingress_ref: &str) -> cwr::CwrResult<()> {
    cwr::v2026_09::local::reject_retired_shared_folder(&workspace_paths.cwr_toml)?;
    let workspace = inspect_local_runtime_workspace(&workspace_paths.cwr_toml)?;
    let default_actor_ref =
        load_optional_default_local_acting_actor_ref(&workspace_paths.cwr_toml)?;
    let mut input = BufReader::new(io::stdin());
    let mut output = io::stdout();

    let selected = wait_for_actor(
        &mut input,
        &mut output,
        &workspace_paths,
        &workspace,
        default_actor_ref,
    )?;
    let HostCommand::SelectActor {
        actor_ref,
        request_id,
        ..
    } = &selected
    else {
        unreachable!("preflight returns only a validated actor selection")
    };
    let executable = env::current_exe()?;
    let connection = connect_or_start_local_resident(
        &workspace_paths,
        Some(actor_ref),
        &executable,
        ingress_ref,
    )
    .and_then(|stream| {
        stream.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "cwp-runtime executable is missing").into()
        })
    });
    let mut stream = match connection {
        Ok(stream) => stream,
        Err(error) => {
            write_event(
                &mut output,
                &HostEvent::CommandFailed {
                    request_id: request_id.clone(),
                    error: error.to_string(),
                },
            )?;
            return Err(error);
        }
    };
    serde_json::to_writer(&mut stream, &selected)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    proxy_streams(stream, input, output)
}

fn wait_for_actor(
    input: &mut impl BufRead,
    output: &mut impl Write,
    workspace_paths: &LocalWorkspacePaths,
    workspace: &cwr::v2026_09::host::HostWorkspace,
    default_actor_ref: Option<String>,
) -> cwr::CwrResult<HostCommand> {
    loop {
        let Some(line) = crate::jsonl::read_delimited_line(input)? else {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "host stdio closed before an acting actor was selected",
            )
            .into());
        };
        let command: HostCommand = serde_json::from_str(&line)?;
        if let Err(error) = command.validate() {
            if command.request_id().trim().is_empty() {
                return Err(error);
            }
            write_event(
                output,
                &HostEvent::CommandFailed {
                    request_id: command.request_id().into(),
                    error: error.to_string(),
                },
            )?;
            continue;
        }
        match &command {
            HostCommand::InspectWorkspace { request_id, .. } => write_event(
                output,
                &HostEvent::WorkspaceInspected {
                    request_id: request_id.clone(),
                    workspace: workspace.clone(),
                    default_actor_ref: default_actor_ref.clone(),
                },
            )?,
            HostCommand::SelectActor {
                request_id,
                actor_ref,
                ..
            } => match load_local_catalog(&workspace_paths.cwr_toml, actor_ref) {
                Ok(_) => return Ok(command),
                Err(error) => write_event(
                    output,
                    &HostEvent::CommandFailed {
                        request_id: request_id.clone(),
                        error: error.to_string(),
                    },
                )?,
            },
            command => write_event(
                output,
                &HostEvent::CommandFailed {
                    request_id: command.request_id().into(),
                    error: "select an acting actor before sending conversation commands".into(),
                },
            )?,
        }
    }
}

pub(crate) fn write_event(output: &mut impl Write, event: &HostEvent) -> cwr::CwrResult<()> {
    serde_json::to_writer(&mut *output, event)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

pub(crate) fn proxy_streams(
    mut stream: UnixStream,
    input: impl Read + Send + 'static,
    mut output: impl Write,
) -> cwr::CwrResult<()> {
    let mut output_stream = stream.try_clone()?;
    thread::Builder::new()
        .name("cwp-runtime-host-input".into())
        .spawn(move || {
            let result = copy_host_input(input, &mut stream);
            if let Err(error) = &result {
                eprintln!("CWP runtime host input ended: {error}");
            }
            let shutdown = if result.is_ok() {
                std::net::Shutdown::Write
            } else {
                std::net::Shutdown::Both
            };
            let _ = stream.shutdown(shutdown);
        })?;
    io::copy(&mut output_stream, &mut output)?;
    Ok(())
}

fn copy_host_input(input: impl Read, output: &mut impl Write) -> cwr::CwrResult<()> {
    let mut input = BufReader::new(input);
    while let Some(frame) = crate::jsonl::read_delimited_bytes(&mut input)? {
        output.write_all(&frame)?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn stdio_proxy_exits_when_the_resident_closes_while_input_remains_open() {
        let (proxy_stream, resident_stream) = UnixStream::pair().unwrap();
        let (input, input_writer) = UnixStream::pair().unwrap();
        let (finished, completion) = mpsc::channel();
        thread::spawn(move || {
            finished
                .send(proxy_streams(proxy_stream, input, Vec::new()).is_ok())
                .unwrap();
        });

        drop(resident_stream);
        assert!(completion.recv_timeout(Duration::from_secs(1)).unwrap());
        drop(input_writer);
    }

    #[test]
    fn host_input_proxy_preserves_a_frame_larger_than_8192_bytes() {
        let mut frame = serde_json::to_vec(&serde_json::json!({
            "kind": "test",
            "text": "x".repeat(16 * 1024),
        }))
        .unwrap();
        frame.push(b'\n');
        let mut output = Vec::new();

        copy_host_input(std::io::Cursor::new(&frame), &mut output).unwrap();

        assert_eq!(output, frame);
    }

    #[test]
    fn host_input_proxy_preserves_crlf_frame_terminators() {
        let frame = b"{\"kind\":\"test\"}\r\n";
        let mut output = Vec::new();

        copy_host_input(std::io::Cursor::new(frame), &mut output).unwrap();

        assert_eq!(output, frame);
    }

    #[test]
    fn host_input_proxy_does_not_forward_an_incomplete_frame() {
        let frame = vec![b'x'; 8192];
        let mut output = Vec::new();

        let error = copy_host_input(std::io::Cursor::new(frame), &mut output).unwrap_err();

        assert!(error.to_string().contains("incomplete JSONL frame"));
        assert!(output.is_empty());
    }
}
