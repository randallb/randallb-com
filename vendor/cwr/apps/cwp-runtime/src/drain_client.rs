use cwr::v2026_09::{
    local::{connect_local_resident, LocalWorkspacePaths},
    service::{RuntimeServiceCommand, RuntimeServiceEvent, RUNTIME_SERVICE_VERSION},
};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    time::Duration,
};

pub(crate) fn run(input: &Path, operation: &str) -> cwr::CwrResult<()> {
    let (socket, directory) = if input.file_name().is_some_and(|name| name == "CWR.toml") {
        let paths = LocalWorkspacePaths::resolve(input)?;
        (paths.runtime_socket_path(), paths.runtime_directory)
    } else {
        let config = crate::config::RuntimeConfig::load(input.to_owned())?;
        (config.socket_path, config.runtime_directory)
    };
    run_for_socket(socket, directory, operation)
}

pub(crate) fn run_for_socket(
    socket: std::path::PathBuf,
    directory: std::path::PathBuf,
    operation: &str,
) -> cwr::CwrResult<()> {
    let marker = directory.join("resident-stopped");
    if operation == "pause-autostart" {
        std::fs::create_dir_all(&directory)?;
        std::fs::write(marker, b"Stopped explicitly from Workloop\n")?;
        println!("0");
        return Ok(());
    }
    if operation == "resume-autostart" {
        match std::fs::remove_file(marker) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        println!("0");
        return Ok(());
    }
    let remaining = request(&socket, operation)?;
    println!("{remaining}");
    Ok(())
}

pub(crate) fn request(socket: &Path, operation: &str) -> cwr::CwrResult<usize> {
    let mut stream = match connect_local_resident(socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(0);
        }
        Err(error) => return Err(error.into()),
    };
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    serde_json::to_writer(
        &mut stream,
        &RuntimeServiceCommand::Drain {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "desktop-drain".into(),
            operation: operation.into(),
        },
    )?;
    stream.write_all(b"\n")?;
    for line in BufReader::new(stream).lines() {
        match serde_json::from_str::<RuntimeServiceEvent>(&line?)? {
            RuntimeServiceEvent::DrainStatus {
                request_id,
                remaining,
            } if request_id == "desktop-drain" => {
                return Ok(remaining);
            }
            RuntimeServiceEvent::CommandFailed { request_id, error }
                if request_id == "desktop-drain" =>
            {
                return Err(std::io::Error::other(error).into())
            }
            _ => {}
        }
    }
    Err(std::io::Error::other("Runtime closed before responding to drain request").into())
}
