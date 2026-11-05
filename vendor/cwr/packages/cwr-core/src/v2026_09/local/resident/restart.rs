/// Called with the generation lock held through verified replacement startup.
fn drain_resident_for_dev_build(
    runtime_directory: &Path,
    generation: &str,
    still_current: impl Fn() -> CwrResult<()>,
) -> CwrResult<()> {
    drain_resident_for_replacement(runtime_directory, Some(generation), still_current)
}

/// Drain and stop an incompatible resident, preserving its persisted workspace state.
/// The caller must hold the resident generation lock until replacement startup finishes.
pub fn replace_incompatible_local_resident(runtime_directory: &Path) -> CwrResult<()> {
    drain_resident_for_replacement(runtime_directory, None, || Ok(()))
}

fn drain_resident_for_replacement(
    runtime_directory: &Path,
    generation: Option<&str>,
    still_current: impl Fn() -> CwrResult<()>,
) -> CwrResult<()> {
    use std::io::BufReader;
    let host = match connect_local_resident(&runtime_directory.join("host.sock")) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(())
        }
        Err(error) => return Err(error.into()),
    };
    let pid = local_resident_peer_pid(&host)?;
    if pid <= 1
        || read_resident_pid(&runtime_directory.join("resident-runtime.pid"))? != Some(pid)
    {
        return invalid("development replacement could not verify the resident PID");
    }
    drop(host);
    let mut reader = BufReader::new(connect_generation_control(runtime_directory, pid)?);
    let supported = generation.is_some() && supports_generation_restart(&mut reader)?;
    let selected_generation = generation.filter(|_| supported);
    if !supported {
        eprintln!("Replacing a runtime using legacy drain control: active tasks will drain, but attached hosts must reconnect manually afterward.");
    }
    let mut committed = false;
    let outcome = (|| {
        let mut remaining = generation_restart_control(&mut reader, selected_generation, "begin")
        .map_err(|error| invalid_error(format!("could not begin development runtime replacement; active work was not stopped: {error}")))?;
        let deadline = Instant::now() + Duration::from_secs(300);
        if remaining != 0 {
            eprintln!(
                "Waiting for {remaining} active tasks before replacing the development runtime."
            );
        }
        while remaining != 0 {
            if Instant::now() >= deadline {
                return invalid(format!("development runtime replacement timed out with {remaining} unfinished tasks; active work was not stopped; retry after it finishes"));
            }
            thread::sleep(Duration::from_millis(100));
            remaining = generation_restart_control(&mut reader, selected_generation, "status")?;
        }
        still_current()?;
        generation_restart_control(&mut reader, selected_generation, "finish")?;
        committed = true;
        if !supported {
            // Legacy finish preserves queued roots but cannot exit with hosts attached.
            // Signal only the same verified process, after it acknowledged zero work.
            still_current()?;
            stop_local_resident_with_expected_pid(
                &runtime_directory.join("resident-runtime.pid"),
                &runtime_directory.join("host.sock"),
                Some(pid),
            )?;
        }
        Ok(())
    })();
    if supported && !committed && outcome.is_err() {
        // Release ownership before the caller releases the generation lock.
        if let Err(error) = generation_restart_control(&mut reader, selected_generation, "cancel") {
            eprintln!("Could not acknowledge development drain cancellation: {error}");
        }
    }
    if !supported && outcome.is_err() {
        // Legacy drains have no connection owner. Undo our fence on normal error
        // paths, including stale builds. A killed requester cannot run this cleanup.
        let cancelled = connect_generation_control(runtime_directory, pid).and_then(|stream| {
            generation_restart_control(&mut BufReader::new(stream), None, "cancel")
        });
        if let Err(error) = cancelled {
            eprintln!("Could not release the older runtime's drain: {error}. If it is still running, use cwp-runtime <workspace> --drain-resident cancel.");
        }
    }
    drop(reader);
    outcome?;
    let deadline = Instant::now() + Duration::from_secs(40);
    while resident_process_running(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if resident_process_running(pid) {
        return invalid(format!(
            "drained resident PID {pid} did not exit; replacement was not started"
        ));
    }
    Ok(())
}

fn connect_generation_control(
    runtime_directory: &Path,
    pid: i32,
) -> CwrResult<std::os::unix::net::UnixStream> {
    let stream = connect_local_resident(&runtime_directory.join("runtime.sock"))?;
    if local_resident_peer_pid(&stream)? != pid {
        return invalid("development replacement sockets belong to different processes");
    }
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(stream)
}

fn resident_supports_tui_graphql(
    host: &std::os::unix::net::UnixStream,
    runtime_directory: &Path,
) -> CwrResult<bool> {
    let pid = local_resident_peer_pid(host)?;
    let mut reader = std::io::BufReader::new(connect_generation_control(runtime_directory, pid)?);
    supports_runtime_capability(&mut reader, "supports-tui-graphql-ingress")
}

fn supports_generation_restart(
    reader: &mut std::io::BufReader<std::os::unix::net::UnixStream>,
) -> CwrResult<bool> {
    supports_runtime_capability(reader, "supports-generation-restart")
}

fn supports_runtime_capability(
    reader: &mut std::io::BufReader<std::os::unix::net::UnixStream>,
    operation: &str,
) -> CwrResult<bool> {
    use super::service::{RuntimeServiceCommand, RuntimeServiceEvent, RUNTIME_SERVICE_VERSION};
    // Old residents reject unknown operations with a correlated response. Sending
    // an unknown command variant instead closes their connection without a reply.
    let command = RuntimeServiceCommand::Drain {
        version: RUNTIME_SERVICE_VERSION,
        request_id: if operation == "supports-generation-restart" {
            "dev-generation-probe"
        } else {
            "tui-graphql-probe"
        }.into(),
        operation: operation.into(),
    };
    match generation_control_response(reader, &command)? {
        RuntimeServiceEvent::DrainStatus { remaining: 0, .. } => Ok(true),
        RuntimeServiceEvent::CommandFailed { error, .. }
            if error == "unknown runtime drain operation" =>
        {
            Ok(false)
        }
        RuntimeServiceEvent::CommandFailed { error, .. } => invalid(error),
        _ => invalid("unexpected development replacement capability response"),
    }
}

fn generation_restart_control(
    reader: &mut std::io::BufReader<std::os::unix::net::UnixStream>,
    generation: Option<&str>,
    operation: &str,
) -> CwrResult<usize> {
    use super::service::{RuntimeServiceCommand, RuntimeServiceEvent, RUNTIME_SERVICE_VERSION};
    let request_id = format!("dev-generation-{operation}");
    let command = if let Some(generation) = generation {
        RuntimeServiceCommand::DevGenerationRestart {
            version: RUNTIME_SERVICE_VERSION,
            request_id,
            operation: operation.into(),
            generation: generation.into(),
        }
    } else {
        RuntimeServiceCommand::Drain {
            version: RUNTIME_SERVICE_VERSION,
            request_id,
            operation: operation.into(),
        }
    };
    match generation_control_response(reader, &command)? {
        RuntimeServiceEvent::DrainStatus { remaining, .. } => Ok(remaining),
        RuntimeServiceEvent::CommandFailed { error, .. } => invalid(error),
        _ => unreachable!("only correlated control responses are returned"),
    }
}

fn generation_control_response(
    reader: &mut std::io::BufReader<std::os::unix::net::UnixStream>,
    command: &super::service::RuntimeServiceCommand,
) -> CwrResult<super::service::RuntimeServiceEvent> {
    use super::service::RuntimeServiceEvent;
    use std::io::{BufRead as _, Write as _};
    serde_json::to_writer(reader.get_mut(), command)?;
    reader.get_mut().write_all(b"\n")?;
    reader.get_mut().flush()?;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return invalid("resident closed before acknowledging development replacement");
        }
        let event: RuntimeServiceEvent = serde_json::from_str(&line)?;
        if matches!(&event, RuntimeServiceEvent::DrainStatus { request_id, .. } | RuntimeServiceEvent::CommandFailed { request_id, .. } if request_id == command.request_id())
        {
            return Ok(event);
        }
    }
}

/// A reconnecting host follows the replacement owner. It never starts or replaces
/// a resident, and never waits indefinitely for that owner's generation lock.
pub fn reconnect_local_resident(
    paths: &LocalWorkspacePaths,
    generation: &str,
    ingress: &str,
) -> CwrResult<std::os::unix::net::UnixStream> {
    reconnect_local_resident_until(paths, generation, ingress, Instant::now() + Duration::from_secs(10))
}

/// Follow a verified replacement within the caller's remaining setup budget.
pub fn reconnect_local_resident_until(
    paths: &LocalWorkspacePaths,
    generation: &str,
    ingress: &str,
    deadline: Instant,
) -> CwrResult<std::os::unix::net::UnixStream> {
    let (mut stream, _generation_lock) =
        reconnect_dev_runtime_generation(&paths.runtime_directory, generation)?;
    let remaining = deadline.checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| invalid_error("replacement workspace binding deadline expired"))?
        .min(Duration::from_secs(2));
    stream.set_read_timeout(Some(remaining))?;
    stream.set_write_timeout(Some(remaining))?;
    bind_local_resident_workspace(
        &mut stream,
        &paths.identity.workspace_id,
        &paths.workspace_directory,
        ingress,
    )?;
    if Instant::now() >= deadline {
        return invalid("replacement workspace binding deadline expired");
    }
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    Ok(stream)
}

/// Follow an already selected development generation without starting a replacement.
pub fn reconnect_dev_runtime_generation(
    runtime_directory: &Path,
    generation: &str,
) -> CwrResult<(std::os::unix::net::UnixStream, File)> {
    let expected = parse_dev_runtime_generation(generation)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(runtime_directory.join("dev-runtime-generation.lock"))?;
    // SAFETY: the file owns this descriptor; flock does not retain it.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    let selected = read_dev_runtime_generation(
        &runtime_directory.join("dev-runtime-generation"),
    )?
    .filter(|current| {
        current.epoch == expected.epoch && current.sequence >= expected.sequence
    })
    .ok_or_else(|| invalid_error("the requested replacement generation is not available"))?;
    let stream = connect_local_resident(&runtime_directory.join("host.sock"))?;
    let pid = local_resident_peer_pid(&stream)?;
    if read_resident_pid(&runtime_directory.join("resident-runtime.pid"))? != Some(pid) {
        return invalid("replacement resident PID does not match its socket");
    }
    // The marker can outlive a failed replacement startup. Verify the socket
    // owner's executable before sending any workspace commands to that peer.
    let executable = process_executable_path(pid)?;
    let announced = fs::read_to_string(runtime_directory.join("dev-runtime-executable"))?;
    if fs::canonicalize(&executable)? != fs::canonicalize(announced.trim_end())? {
        return invalid("replacement resident belongs to a different runtime bundle");
    }
    verify_dev_runtime_executable(&executable, &selected).map_err(|error| {
        invalid_error(format!("replacement resident executable does not match the requested generation: {error}"))
    })?;
    Ok((stream, lock))
}
