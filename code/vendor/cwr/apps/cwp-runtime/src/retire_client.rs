use cwr::v2026_09::{
    local::{connect_local_resident, LocalWorkspacePaths},
    service::{RuntimeServiceCommand, RuntimeServiceEvent, RUNTIME_SERVICE_VERSION},
};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    time::Duration,
};

pub(crate) fn run(input: &Path) -> cwr::CwrResult<()> {
    let socket = if input.file_name().is_some_and(|name| name == "CWR.toml") {
        LocalWorkspacePaths::resolve(input)?.runtime_socket_path()
    } else {
        crate::config::RuntimeConfig::load(input.to_owned())?.socket_path
    };
    run_socket(&socket)
}

pub(crate) fn run_socket(socket: &Path) -> cwr::CwrResult<()> {
    let mut stream = match connect_local_resident(socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(())
        }
        Err(error) => return Err(error.into()),
    };
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request_id = "exit-resident-when-idle";
    serde_json::to_writer(
        &mut stream,
        &RuntimeServiceCommand::ExitWhenIdle {
            version: RUNTIME_SERVICE_VERSION,
            request_id: request_id.into(),
        },
    )?;
    stream.write_all(b"\n")?;
    for line in BufReader::new(stream).lines() {
        match serde_json::from_str::<RuntimeServiceEvent>(&line?)? {
            RuntimeServiceEvent::CommandSucceeded {
                request_id: response,
                ..
            } if response == request_id => return Ok(()),
            RuntimeServiceEvent::CommandFailed {
                request_id: response,
                error,
            } if response == request_id => return Err(std::io::Error::other(error).into()),
            _ => {}
        }
    }
    Err(std::io::Error::other("Runtime closed before accepting idle exit").into())
}
