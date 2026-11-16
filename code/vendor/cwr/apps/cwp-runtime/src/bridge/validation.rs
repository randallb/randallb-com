fn write_command(
    writer: &Arc<Mutex<UnixStream>>,
    command: &RuntimeServiceCommand,
) -> CwrResult<()> {
    let mut writer = writer
        .lock()
        .map_err(|_| io::Error::other("runtime bridge writer was poisoned"))?;
    serde_json::to_writer(&mut *writer, command)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn write_host_event(output: &SharedHostWriter, event: &HostEvent) -> CwrResult<()> {
    output::write_event(output, event)
}

fn source_ref(runtime_ref: &str) -> String {
    format!("{runtime_ref}/sources/conversations")
}

fn require_source(expected: &str, actual: &str) -> CwrResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown conversation source {actual}"),
        )
        .into())
    }
}

fn accept_command_check(
    output: &SharedHostWriter,
    request_id: &str,
    result: CwrResult<()>,
) -> CwrResult<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(error) => {
            write_host_event(
                output,
                &HostEvent::CommandFailed {
                    request_id: request_id.into(),
                    error: error.to_string(),
                },
            )?;
            Ok(false)
        }
    }
}

fn accept_claimed_actor(
    output: &SharedHostWriter,
    request_id: &str,
    bound_actor_ref: Option<&str>,
    claimed_actor_ref: &str,
) -> CwrResult<bool> {
    let Some(bound_actor_ref) = bound_actor_ref else {
        return Ok(true);
    };
    if claimed_actor_ref == bound_actor_ref {
        return Ok(true);
    }
    write_host_event(
        output,
        &HostEvent::CommandFailed {
            request_id: request_id.into(),
            error: format!(
                "host session actor {bound_actor_ref} cannot submit as {claimed_actor_ref}"
            ),
        },
    )?;
    Ok(false)
}


