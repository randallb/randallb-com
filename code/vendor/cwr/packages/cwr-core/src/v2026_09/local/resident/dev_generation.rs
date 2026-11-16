const DEV_RUNTIME_GENERATION_ENV: &str = "CWR_DEV_RUNTIME_GENERATION";
const DEV_RUNTIME_EXECUTABLE_ENV: &str = "CWR_DEV_RUNTIME_EXECUTABLE";
const DEV_RUNTIME_GENERATION_FILE_ENV: &str = "CWR_DEV_RUNTIME_GENERATION_FILE";

struct DevRuntimeGeneration {
    value: String,
    epoch: u64,
    sequence: u64,
    digest: String,
    size: u64,
}

fn prepare_dev_runtime_generation(
    workspace_paths: &LocalWorkspacePaths,
    executable: &Path,
) -> CwrResult<Option<File>> {
    prepare_dev_runtime_generation_for_resident(&workspace_paths.runtime_directory, executable)
}

/// Fence one resident directory to the development runtime generation selected by the shim.
pub fn prepare_dev_runtime_generation_for_resident(
    runtime_directory: &Path,
    executable: &Path,
) -> CwrResult<Option<File>> {
    let lock = acquire_dev_generation_lock(
        &runtime_directory.join("dev-runtime-generation.lock"),
    )?;
    let Some(value) = std::env::var_os(DEV_RUNTIME_GENERATION_ENV) else {
        return Ok(Some(lock));
    };
    let value = value
        .into_string()
        .map_err(|_| invalid_error("invalid development runtime generation"))?;
    let generation = parse_dev_runtime_generation(&value)?;
    let announced_executable = std::env::var_os(DEV_RUNTIME_EXECUTABLE_ENV)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| invalid_error("invalid development runtime executable"))?;
    let repository_generation_path = std::env::var_os(DEV_RUNTIME_GENERATION_FILE_ENV)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| invalid_error("invalid development runtime generation file"))?;
    if fs::canonicalize(&announced_executable)? != fs::canonicalize(executable)? {
        return invalid("development runtime executable does not match the running bridge");
    }

    let generation_path = runtime_directory.join("dev-runtime-generation");
    ensure_dev_runtime_generation_current(&repository_generation_path, executable, &generation)?;
    let recorded = read_dev_runtime_generation(&generation_path)?;
    if recorded
        .as_ref()
        .is_some_and(|recorded| recorded.value == generation.value)
        && resident_uses_dev_executable(runtime_directory, executable)?
    {
        write_dev_runtime_generation(
            &runtime_directory.join("dev-runtime-executable"),
            fs::canonicalize(executable)?.to_str().ok_or_else(|| invalid_error("invalid runtime executable path"))?,
        )?;
        return Ok(Some(lock));
    }
    drain_resident_for_dev_build(runtime_directory, &generation.value, || {
        ensure_dev_runtime_generation_current(&repository_generation_path, executable, &generation)
    })?;
    ensure_dev_runtime_generation_current(&repository_generation_path, executable, &generation)?;
    write_dev_runtime_generation(
        &runtime_directory.join("dev-runtime-executable"),
        fs::canonicalize(executable)?.to_str().ok_or_else(|| invalid_error("invalid runtime executable path"))?,
    )?;
    write_dev_runtime_generation(&generation_path, &generation.value)?;
    Ok(Some(lock))
}

fn ensure_dev_runtime_generation_current(
    repository_generation_path: &Path,
    executable: &Path,
    generation: &DevRuntimeGeneration,
) -> CwrResult<()> {
    let repository_generation = read_dev_runtime_generation(repository_generation_path)?;
    if repository_generation
        .as_ref()
        .is_none_or(|current| current.value != generation.value)
    {
        return invalid(
            "development runtime was rebuilt again while this command was starting; retry the command",
        );
    }
    verify_dev_runtime_executable(executable, generation)
}

fn parse_dev_runtime_generation(value: &str) -> CwrResult<DevRuntimeGeneration> {
    let fields = value.split_ascii_whitespace().collect::<Vec<_>>();
    if fields.len() != 4
        || !fields[0..2]
            .iter()
            .all(|field| field.bytes().all(|byte| byte.is_ascii_digit()))
        || fields[2].len() != 64
        || !fields[2]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || !fields[3].bytes().all(|byte| byte.is_ascii_digit())
    {
        return invalid("invalid development runtime generation");
    }
    Ok(DevRuntimeGeneration {
        value: value.into(),
        epoch: fields[0].parse()?,
        sequence: fields[1].parse()?,
        digest: fields[2].into(),
        size: fields[3].parse()?,
    })
}

fn read_dev_runtime_generation(path: &Path) -> CwrResult<Option<DevRuntimeGeneration>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(parse_dev_runtime_generation(value.trim_end()).ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn verify_dev_runtime_executable(path: &Path, generation: &DevRuntimeGeneration) -> CwrResult<()> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() != generation.size {
        return invalid("development runtime executable changed while this command was starting");
    }
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    if hex::encode(digest.finalize()) != generation.digest {
        return invalid("development runtime executable changed while this command was starting");
    }
    Ok(())
}

fn write_dev_runtime_generation(path: &Path, value: &str) -> CwrResult<()> {
    let temporary = path.with_file_name(format!(".dev-runtime-generation.{}.tmp", Uuid::new_v4()));
    let result = (|| -> CwrResult<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&temporary)?;
        std::io::Write::write_all(&mut file, format!("{value}\n").as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn resident_uses_dev_executable(
    runtime_directory: &Path,
    executable: &Path,
) -> CwrResult<bool> {
    let pid_path = runtime_directory.join("resident-runtime.pid");
    let Some(pid) = read_resident_pid(&pid_path)? else {
        return Ok(false);
    };
    if !resident_process_running(pid) {
        return Ok(false);
    }
    let stream = match connect_local_resident(&runtime_directory.join("host.sock")) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error.into()),
    };
    if local_resident_peer_pid(&stream)? != pid {
        return Ok(false);
    }
    let resident_executable = match fs::canonicalize(process_executable_path(pid)?) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    Ok(resident_executable == fs::canonicalize(executable)?)
}

#[cfg(target_os = "linux")]
fn process_executable_path(pid: i32) -> io::Result<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe"))
}

#[cfg(target_vendor = "apple")]
fn process_executable_path(pid: i32) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    let mut buffer = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is writable for the supplied length and proc_pidpath does not retain it.
    let length =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if length <= 0 {
        return Err(io::Error::last_os_error());
    }
    buffer.truncate(length as usize);
    Ok(PathBuf::from(std::ffi::OsString::from_vec(buffer)))
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
fn process_executable_path(_pid: i32) -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "resident executable inspection is unavailable on this platform",
    ))
}

fn acquire_dev_generation_lock(path: &Path) -> CwrResult<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    loop {
        // SAFETY: `file` owns the descriptor throughout this call. flock neither takes
        // ownership nor retains the descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(file);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    }
}

/// Stop the process verified by the private resident socket and PID file.
pub fn stop_local_resident(pid_path: &Path, socket_path: &Path) -> CwrResult<()> {
    stop_local_resident_with_expected_pid(pid_path, socket_path, None)
}

/// Stop the verified resident only when its socket belongs to `expected_pid`.
pub fn stop_local_resident_if_pid(
    pid_path: &Path,
    socket_path: &Path,
    expected_pid: i32,
) -> CwrResult<()> {
    stop_local_resident_with_expected_pid(pid_path, socket_path, Some(expected_pid))
}

fn stop_local_resident_with_expected_pid(
    pid_path: &Path,
    socket_path: &Path,
    expected_pid: Option<i32>,
) -> CwrResult<()> {
    let recorded_pid = read_resident_pid(pid_path)?;
    let stream = match connect_local_resident(socket_path) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            if recorded_pid.is_some_and(|pid| !resident_process_running(pid)) {
                let _ = fs::remove_file(pid_path);
            }
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let peer_pid = local_resident_peer_pid(&stream)?;
    if peer_pid <= 1 {
        return invalid(format!("invalid resident socket peer PID {peer_pid}"));
    }
    if expected_pid.is_some_and(|expected| expected != peer_pid) {
        return invalid("resident changed while draining; refusing to stop another process");
    }
    if recorded_pid.is_some_and(|pid| resident_process_running(pid) && pid != peer_pid) {
        return invalid(format!(
            "refusing to stop PID {peer_pid}: the resident PID file names another running process"
        ));
    }
    let pid = peer_pid;
    // SAFETY: the positive PID came from this workspace's owner-only runtime directory,
    // and the private socket independently identified the same process.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while resident_process_running(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    if resident_process_running(pid) {
        return invalid(format!(
            "resident CWR runtime PID {pid} did not stop after SIGTERM"
        ));
    }
    let _ = fs::remove_file(pid_path);
    Ok(())
}

fn read_resident_pid(path: &Path) -> CwrResult<Option<i32>> {
    let value = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let Ok(pid) = value.trim().parse::<i32>() else {
        let _ = fs::remove_file(path);
        return Ok(None);
    };
    if pid <= 1 {
        let _ = fs::remove_file(path);
        return Ok(None);
    }
    Ok(Some(pid))
}

fn resident_process_running(pid: i32) -> bool {
    // SAFETY: signal 0 performs existence/permission checking without delivering a signal.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(target_vendor = "apple")]
fn local_resident_peer_pid(stream: &std::os::unix::net::UnixStream) -> io::Result<i32> {
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of_val(&pid) as libc::socklen_t;
    // SAFETY: the socket descriptor is live, and pid and length are writable buffers of
    // the exact size required by LOCAL_PEERPID.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    } == 0
    {
        Ok(pid)
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn local_resident_peer_pid(stream: &std::os::unix::net::UnixStream) -> io::Result<i32> {
    // SAFETY: ucred is a C POD type populated by getsockopt.
    let mut credentials = unsafe { std::mem::zeroed::<libc::ucred>() };
    let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: the socket descriptor is live and the credential buffers are valid for writes.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    } == 0
    {
        Ok(credentials.pid)
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
fn local_resident_peer_pid(_stream: &std::os::unix::net::UnixStream) -> io::Result<i32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "resident peer PID inspection is unavailable on this platform",
    ))
}
