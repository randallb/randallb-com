use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fs, io,
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use cwr::v2026_09::local::{bind_local_resident_workspace, LocalWorkspacePaths};

const CLI_INGRESS_INTEGRATION_REF: &str = "cwp://org.coworkerprotocol.local/integrations/cli";
pub(super) fn run_runtime_command(
    arguments: &[OsString],
    cwr_toml: &Path,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let paths = ResidentPaths::new(cwr_toml)?;
    match arguments {
        [command] if command == OsStr::new("status") => status(&paths),
        [command] if command == OsStr::new("stop") => stop(&paths, true),
        _ => Err(
            "usage: cwr [--workspace <workspace-directory|CWR.toml>] runtime <status|stop>".into(),
        ),
    }
}

struct ResidentPaths {
    pid: PathBuf,
    host_socket: PathBuf,
    workspace_id: String,
    workspace_directory: PathBuf,
}

impl ResidentPaths {
    fn new(cwr_toml: &Path) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let workspace = LocalWorkspacePaths::resolve(cwr_toml)?;
        Ok(Self {
            pid: workspace.resident_pid_path(),
            host_socket: workspace.host_socket_path(),
            workspace_id: workspace.identity.workspace_id,
            workspace_directory: workspace.workspace_directory,
        })
    }
}

fn status(paths: &ResidentPaths) -> Result<(), Box<dyn Error + Send + Sync>> {
    let Some(pid) = read_pid(&paths.pid)? else {
        println!("Resident CWR runtime is not running.");
        return Ok(());
    };
    if process_running(pid) && verified_resident_peer(paths)? == pid {
        println!("Resident CWR runtime is running (PID {pid}).");
    } else {
        println!("Resident CWR runtime is not running (stale PID {pid}).");
    }
    Ok(())
}

fn stop(paths: &ResidentPaths, report: bool) -> Result<(), Box<dyn Error + Send + Sync>> {
    let Some(pid) = read_pid(&paths.pid)? else {
        if report {
            println!("Resident CWR runtime is not running.");
        }
        return Ok(());
    };
    if !process_running(pid) {
        let _ = fs::remove_file(&paths.pid);
        if report {
            println!("Resident CWR runtime is not running (removed stale PID {pid}).");
        }
        return Ok(());
    }
    match verified_resident_peer(paths) {
        Ok(peer_pid) if peer_pid == pid => {}
        Ok(peer_pid) => {
            return Err(format!(
                "refusing to stop PID {pid}: the resident host socket belongs to PID {peer_pid}"
            )
            .into())
        }
        Err(error) => {
            return Err(format!(
                "refusing to stop PID {pid}: could not verify the resident host socket: {error}"
            )
            .into())
        }
    }
    // SAFETY: the validated positive PID came from this workspace's owner-only
    // resident-runtime.pid, and SIGTERM requests the runtime's managed shutdown.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_running(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if process_running(pid) {
        return Err(format!("resident CWR runtime PID {pid} did not stop after SIGTERM").into());
    }
    let _ = fs::remove_file(&paths.pid);
    if report {
        println!("Stopped resident CWR runtime PID {pid}.");
    }
    Ok(())
}

fn read_pid(path: &Path) -> Result<Option<i32>, Box<dyn Error + Send + Sync>> {
    let value = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let pid: i32 = value.trim().parse()?;
    if pid <= 1 {
        return Err(format!("invalid resident runtime PID {pid} in {}", path.display()).into());
    }
    Ok(Some(pid))
}

fn process_running(pid: i32) -> bool {
    // SAFETY: signal 0 performs existence/permission checking without delivering a signal.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
fn resident_peer_pid(path: &Path) -> io::Result<i32> {
    let stream = UnixStream::connect(path)?;
    peer_pid(&stream)
}

fn verified_resident_peer(paths: &ResidentPaths) -> Result<i32, Box<dyn Error + Send + Sync>> {
    let mut stream = UnixStream::connect(&paths.host_socket)?;
    let pid = peer_pid(&stream)?;
    bind_local_resident_workspace(
        &mut stream,
        &paths.workspace_id,
        &paths.workspace_directory,
        CLI_INGRESS_INTEGRATION_REF,
    )?;
    Ok(pid)
}

#[cfg(target_vendor = "apple")]
fn peer_pid(stream: &UnixStream) -> io::Result<i32> {
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of_val(&pid) as libc::socklen_t;
    // SAFETY: the socket descriptor is live, and pid and length are writable
    // buffers of the exact size required by LOCAL_PEERPID.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    };
    if result == 0 {
        Ok(pid)
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn peer_pid(stream: &UnixStream) -> io::Result<i32> {
    // SAFETY: ucred is a C POD type whose fields are populated by getsockopt.
    let mut credentials = unsafe { std::mem::zeroed::<libc::ucred>() };
    let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: the socket descriptor is live, and credentials and length are
    // writable buffers of the exact size required by SO_PEERCRED.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result == 0 {
        Ok(credentials.pid)
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
fn peer_pid(_stream: &UnixStream) -> io::Result<i32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "peer PID inspection is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    #[test]
    fn missing_pid_file_means_no_resident_runtime() {
        let temporary = tempfile::tempdir().unwrap();
        assert_eq!(
            read_pid(&temporary.path().join("missing.pid")).unwrap(),
            None
        );
    }

    #[test]
    fn pid_file_requires_a_process_pid() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("resident-runtime.pid");
        for invalid in ["not-a-pid", "0", "1", "-42"] {
            fs::write(&path, invalid).unwrap();
            assert!(read_pid(&path).is_err(), "{invalid} must be rejected");
        }
    }

    #[test]
    fn pid_file_accepts_a_positive_process_pid() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("resident-runtime.pid");
        fs::write(&path, "4242\n").unwrap();
        assert_eq!(read_pid(&path).unwrap(), Some(4242));
    }

    #[test]
    fn resident_socket_identifies_its_server_process() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("host.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        assert_eq!(
            resident_peer_pid(&path).unwrap(),
            i32::try_from(std::process::id()).unwrap()
        );
    }

    #[test]
    fn resident_verification_rejects_another_workspace_binding() {
        let temporary = tempfile::tempdir().unwrap();
        let socket = temporary.path().join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let handshake: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(
                handshake["ingress_integration_ref"],
                CLI_INGRESS_INTEGRATION_REF
            );
            serde_json::to_writer(
                &mut stream,
                &serde_json::json!({
                    "kind": "workspace_rejected",
                    "version": 1,
                    "error": "resident runtime owns another workspace path"
                }),
            )
            .unwrap();
            stream.write_all(b"\n").unwrap();
        });
        let paths = ResidentPaths {
            pid: temporary.path().join("resident.pid"),
            host_socket: socket,
            workspace_id: "workspace-one".into(),
            workspace_directory: temporary.path().into(),
        };

        let error = verified_resident_peer(&paths).unwrap_err().to_string();

        assert!(error.contains("another workspace path"));
        server.join().unwrap();
    }
}
