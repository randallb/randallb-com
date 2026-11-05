use super::*;

pub(super) fn start(prepared: &PreparedConfiguredRuntime, socket: &Path) -> CwrResult<UnixStream> {
    use std::{
        os::unix::process::CommandExt,
        process::{Command, Stdio},
    };
    let selected = prepared.selected();
    if selected
        .runtime_directory()?
        .join("resident-stopped")
        .exists()
    {
        return Err(io::Error::other(
            "Background work is stopped; resume-autostart before reconnecting",
        )
        .into());
    }
    let log_path = selected.state_directory.join("diagnostics/wlpdb.sqlite3");
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(selected.workspace_path())
        .args(["--runtime", &selected.name, "--resident"])
        .current_dir(&selected.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(
            "CWR_RESIDENT_DIAGNOSTIC_STATE_DIRECTORY",
            &selected.state_directory,
        );
    // SAFETY: setsid runs only in the forked child before exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    wait_for_started_resident(command.spawn()?, socket, &log_path)
}

pub(super) fn wait_for_started_resident(
    mut child: std::process::Child,
    socket: &Path,
    log_path: &Path,
) -> CwrResult<UnixStream> {
    use std::time::Duration;

    // Retained state can take longer than a fixed startup window to load.
    // Keep waiting while the child is alive and the socket is not ready.
    loop {
        match cwr::v2026_09::local::connect_local_resident(socket) {
            Ok(stream) => {
                // A competing starter may own this socket after our child exits.
                // The configured handshake verifies the connected resident.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
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
                        "configured runtime resident exited ({status}); see {}",
                        log_path.display()
                    ))
                    .into());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "configured runtime resident did not start; see {}: {error}",
                        log_path.display()
                    ),
                )
                .into());
            }
        }
    }
}
