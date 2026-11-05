fn canonical_cwr_path(path: &Path) -> CwrResult<PathBuf> {
    let path = if path.is_dir() {
        path.join("CWR.toml")
    } else {
        path.to_owned()
    };
    if path.file_name().and_then(|name| name.to_str()) != Some("CWR.toml") {
        crate::v2026_09::graph::validate_runtime_definition(&path)?;
    }
    fs::canonicalize(path).map_err(Into::into)
}

fn validate_local_host_identity(identity: &LocalHostIdentity) -> CwrResult<()> {
    CwrRef::parse(&identity.acting_actor_ref).map_err(|error| {
        invalid_error(format!(
            "invalid local acting actor ref {}: {error}",
            identity.acting_actor_ref
        ))
    })?;
    let ingress = CwrRef::parse(&identity.ingress_integration_ref).map_err(|error| {
        invalid_error(format!(
            "invalid local ingress integration ref {}: {error}",
            identity.ingress_integration_ref
        ))
    })?;
    if identity.ingress_integration_ref.starts_with("~@") || ingress.fragment().is_some() {
        return invalid("local ingress integration ref must be absolute and unfragmented");
    }
    Ok(())
}

fn canonical_local_state_path(state_path: PathBuf) -> CwrResult<PathBuf> {
    let file_name = state_path
        .file_name()
        .ok_or_else(|| invalid_error("local runtime state path has no file name"))?
        .to_owned();
    let parent = state_path
        .parent()
        .ok_or_else(|| invalid_error("local runtime state path has no parent directory"))?;
    fs::create_dir_all(parent)?;
    Ok(fs::canonicalize(parent)?.join(file_name))
}

fn acquire_local_state_lease(state_path: &Path) -> CwrResult<File> {
    let lock_path = state_path.with_extension("lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&lock_path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    loop {
        // SAFETY: the descriptor remains owned by `file` for the duration of `flock`, and the
        // operation neither takes ownership of nor retains the descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(file);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        if error
            .raw_os_error()
            .is_some_and(|code| code == libc::EWOULDBLOCK || code == libc::EAGAIN)
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!(
                    "local CWR state {} is already in use by another process; wait for it to finish and retry",
                    state_path.display()
                ),
            )
            .into());
        }
        return Err(io::Error::new(
            error.kind(),
            format!(
                "could not lock local CWR state {}: {error}",
                state_path.display()
            ),
        )
        .into());
    }
}

fn path_derived_runtime_root_ref(cwr_toml: &Path) -> String {
    let digest = Sha256::digest(cwr_toml.to_string_lossy().as_bytes());
    format!(
        "cwp://org.coworkerprotocol.local/runtimes/{}",
        hex::encode(digest)
    )
}

fn new_invocation_ref(runtime_root_ref: &str) -> String {
    format!("{runtime_root_ref}/invocations/{}", Uuid::new_v4().simple())
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(invalid_error(message))
}

fn invalid_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(crate::ConfigurationError(message.into()))
}
