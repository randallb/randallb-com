/// Local owner completion evidence. The caller must retain its fresh nonce and
/// hold exclusive controller ownership/autostart pause through replacement.
/// This proves completion of this owner, not absence of a replacement owner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalResidentRetirement {
    pub version: u32,
    pub nonce: String,
    pub runtime_ref: String,
    pub owner_pid: i32,
    pub owner_uid: u32,
}

impl LocalResidentRetirement {
    pub fn validate(&self) -> CwrResult<()> {
        let nonce = Uuid::parse_str(&self.nonce)?;
        if self.version != 1
            || nonce.is_nil()
            || nonce.to_string() != self.nonce
            || self.owner_pid <= 1
            || !self.runtime_ref.starts_with("cwp://")
            || self.runtime_ref.len() > 2048
            || self.runtime_ref.chars().any(char::is_whitespace)
        {
            return invalid("invalid resident retirement identity");
        }
        Ok(())
    }
}

/// Called by the resident only after its runloop, managed shutdown and every
/// communication adapter reap succeeded. Never publish from a destructor.
/// A visible exact receipt proves that teardown completed even if a later
/// directory sync or process-exit step fails. Publication attempts file/parent
/// sync; this is not physical power-loss certification or a process exit code.
pub fn publish_completed_local_resident_retirement(
    directory: &Path,
    receipt: &LocalResidentRetirement,
) -> CwrResult<()> {
    use std::io::Write as _;
    receipt.validate()?;
    if receipt.owner_pid != std::process::id() as i32
        || receipt.owner_uid != resident_retirement_uid()
    {
        return invalid("retirement receipt does not identify this owner");
    }
    let temporary = directory.join(format!(".retirement-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        serde_json::to_writer(&mut file, receipt)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, directory.join("resident-retirement-v1.json"))?;
        File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Exact known-operation lookup. An absent or different latest receipt is
/// unknown; it is never adopted as authorization for a new update operation.
pub fn completed_local_resident_retirement(
    directory: &Path,
    expected: &LocalResidentRetirement,
) -> CwrResult<bool> {
    use std::{io::Read as _, os::unix::fs::MetadataExt as _};
    expected.validate()?;
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(directory.join("resident-retirement-v1.json"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.len() > 8192
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != resident_retirement_uid()
    {
        return invalid("invalid resident retirement receipt file");
    }
    let mut bytes = Vec::new();
    file.by_ref().take(8193).read_to_end(&mut bytes)?;
    if bytes.len() > 8192 {
        return invalid("resident retirement receipt is too large");
    }
    let receipt: LocalResidentRetirement = serde_json::from_slice(&bytes)?;
    receipt.validate()?;
    Ok(receipt == *expected && !resident_process_running(expected.owner_pid))
}

/// One attempt against an existing configured owner. Persist a fresh random
/// nonce before calling. A timeout, lost acknowledgement or disconnect can
/// follow commitment: preserve exclusivity/pause and recover the exact tuple.
/// Busy work is rejected without commitment; this function does not retry it.
pub fn retire_existing_local_resident(
    directory: &Path,
    expected: &LocalResidentRetirement,
) -> CwrResult<()> {
    request_existing_local_resident_retirement(directory, expected, false)
}

/// Register an exact operation before Desktop closes its own UI lease. This
/// does not fence work or promise shutdown. Only later zero-work commitment and
/// successful teardown may publish its receipt; a crash remains unknown.
pub fn arm_existing_local_resident_retirement(
    directory: &Path,
    expected: &LocalResidentRetirement,
) -> CwrResult<()> {
    request_existing_local_resident_retirement(directory, expected, true)
}

fn request_existing_local_resident_retirement(
    directory: &Path,
    expected: &LocalResidentRetirement,
    arm: bool,
) -> CwrResult<()> {
    use super::service::{RuntimeServiceCommand, RuntimeServiceEvent, RUNTIME_SERVICE_VERSION};
    use std::io::{BufReader, Write as _};
    expected.validate()?;
    if !arm && completed_local_resident_retirement(directory, expected)? {
        return Ok(());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = resident_observation_connect(&directory.join("runtime.sock"), deadline)?;
    let host = resident_observation_connect(&directory.join("host.sock"), deadline)?;
    if local_resident_peer_pid(&stream)? != expected.owner_pid
        || local_resident_peer_pid(&host)? != expected.owner_pid
        || expected.owner_uid != resident_retirement_uid()
    {
        return invalid("retirement owner changed");
    }
    drop(host);
    let mut reader = BufReader::new(stream.try_clone()?);
    match resident_observation_frame(&mut reader, deadline)? {
        RuntimeServiceEvent::Ready { runtime_ref } if runtime_ref == expected.runtime_ref => {}
        _ => return invalid("retirement runtime identity changed"),
    }
    let request_id = format!("retire-{}", Uuid::new_v4());
    for command in [
        RuntimeServiceCommand::Drain {
            version: RUNTIME_SERVICE_VERSION,
            request_id: request_id.clone(),
            operation: if arm {
                "supports-configured-retirement-arm-v1"
            } else {
                "supports-configured-retirement-v1"
            }
            .into(),
        },
        if arm {
            RuntimeServiceCommand::ArmConfiguredResident {
                version: RUNTIME_SERVICE_VERSION,
                request_id: request_id.clone(),
                nonce: expected.nonce.clone(),
                expected_runtime_ref: expected.runtime_ref.clone(),
                expected_owner_pid: expected.owner_pid,
            }
        } else {
            RuntimeServiceCommand::RetireConfiguredResident {
                version: RUNTIME_SERVICE_VERSION,
                request_id: request_id.clone(),
                nonce: expected.nonce.clone(),
                expected_runtime_ref: expected.runtime_ref.clone(),
                expected_owner_pid: expected.owner_pid,
            }
        },
    ] {
        stream.set_write_timeout(Some(resident_observation_remaining(deadline)?))?;
        serde_json::to_writer(&mut stream, &command)?;
        stream.write_all(b"\n")?;
        let event = resident_observation_frame(&mut reader, deadline)?;
        let valid = match (&command, event) {
            (
                RuntimeServiceCommand::Drain { .. },
                RuntimeServiceEvent::DrainStatus {
                    request_id: response,
                    remaining: 0,
                },
            ) => response == request_id,
            (
                RuntimeServiceCommand::RetireConfiguredResident { .. }
                | RuntimeServiceCommand::ArmConfiguredResident { .. },
                RuntimeServiceEvent::CommandSucceeded {
                    request_id: response,
                    ..
                },
            ) => response == request_id,
            (
                _,
                RuntimeServiceEvent::CommandFailed {
                    request_id: response,
                    error,
                },
            ) if response == request_id => return invalid(error),
            _ => false,
        };
        if !valid {
            return invalid("retirement was not acknowledged; outcome may be committed");
        }
    }
    drop(reader);
    drop(stream);
    if arm {
        return Ok(());
    }
    let completion_deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if completed_local_resident_retirement(directory, expected)? {
            return Ok(());
        }
        if Instant::now() >= completion_deadline {
            return invalid("retirement completion is unverified; preserve controller ownership");
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn resident_retirement_uid() -> u32 {
    // SAFETY: geteuid has no arguments or pointers and only reads process identity.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod retirement_receipt_tests {
    use super::*;
    fn identity() -> LocalResidentRetirement {
        LocalResidentRetirement {
            version: 1,
            nonce: Uuid::new_v4().to_string(),
            runtime_ref: "cwp://local.fixture/runtimes/one".into(),
            owner_pid: std::process::id() as i32,
            owner_uid: resident_retirement_uid(),
        }
    }
    #[test]
    fn receipt_is_private_exact_and_never_proves_a_live_owner_stopped() {
        let root = tempfile::tempdir().unwrap();
        let expected = identity();
        assert!(!completed_local_resident_retirement(root.path(), &expected).unwrap());
        publish_completed_local_resident_retirement(root.path(), &expected).unwrap();
        assert_eq!(
            fs::metadata(root.path().join("resident-retirement-v1.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
        assert!(!completed_local_resident_retirement(root.path(), &expected).unwrap());
        let mut other = expected.clone();
        other.nonce = Uuid::new_v4().to_string();
        assert!(!completed_local_resident_retirement(root.path(), &other).unwrap());
        other.owner_pid = 1;
        assert!(publish_completed_local_resident_retirement(root.path(), &other).is_err());
        assert_eq!(
            serde_json::from_slice::<LocalResidentRetirement>(
                &fs::read(root.path().join("resident-retirement-v1.json")).unwrap()
            )
            .unwrap(),
            expected
        );
    }
    #[test]
    fn receipt_fifo_fails_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt as _;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("resident-retirement-v1.json");
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: c_path is a valid nul-terminated pathname; mode is private.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let started = Instant::now();
        assert!(completed_local_resident_retirement(root.path(), &identity()).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn autostart_pause_is_idempotent_and_corrupt_markers_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(set_local_resident_autostart_paused(root.path(), true).is_err());
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!local_resident_autostart_paused(root.path()).unwrap());
        set_local_resident_autostart_paused(root.path(), true).unwrap();
        assert!(local_resident_autostart_paused(root.path()).unwrap());
        set_local_resident_autostart_paused(root.path(), true).unwrap();
        set_local_resident_autostart_paused(root.path(), false).unwrap();
        assert!(!local_resident_autostart_paused(root.path()).unwrap());
        std::os::unix::fs::symlink(
            root.path().join("missing"),
            root.path().join("resident-stopped"),
        )
        .unwrap();
        assert!(set_local_resident_autostart_paused(root.path(), false).is_err());
        assert!(fs::symlink_metadata(root.path().join("resident-stopped"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn malformed_oversized_and_symlink_receipts_are_preserved_and_rejected() {
        let root = tempfile::tempdir().unwrap();
        let expected = identity();
        let path = root.path().join("resident-retirement-v1.json");
        fs::write(&path, "not a receipt").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(completed_local_resident_retirement(root.path(), &expected).is_err());
        fs::write(&path, vec![b'x'; 8193]).unwrap();
        assert!(completed_local_resident_retirement(root.path(), &expected).is_err());
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(root.path().join("missing"), &path).unwrap();
        assert!(completed_local_resident_retirement(root.path(), &expected).is_err());
        assert!(fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
    }
}

/// Supported autostart control for an exclusively managed configured directory.
/// The controller journals the prior value before changing it and preserves the
/// pause after an unknown retirement result. This does not stop a live owner.
pub fn local_resident_autostart_paused(directory: &Path) -> CwrResult<bool> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = match fs::symlink_metadata(directory.join("resident-stopped")) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file()
        || metadata.uid() != resident_retirement_uid()
        || metadata.mode() & 0o022 != 0
    {
        return invalid("resident autostart marker requires review");
    }
    Ok(true)
}

pub fn set_local_resident_autostart_paused(directory: &Path, paused: bool) -> CwrResult<()> {
    use std::{io::Write as _, os::unix::fs::MetadataExt as _};
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir()
        || metadata.uid() != resident_retirement_uid()
        || metadata.mode() & 0o077 != 0
    {
        return invalid("resident autostart directory is not private");
    }
    let prior = local_resident_autostart_paused(directory)?;
    if prior == paused {
        if paused {
            File::open(directory.join("resident-stopped"))?.sync_all()?;
        }
        File::open(directory)?.sync_all()?;
        return Ok(());
    }
    let marker = directory.join("resident-stopped");
    if paused {
        let temporary = directory.join(format!(".autostart-{}.tmp", Uuid::new_v4()));
        let result = (|| -> CwrResult<()> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(b"Stopped explicitly from Workloop\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &marker)?;
            File::open(directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    } else {
        fs::remove_file(marker)?;
        File::open(directory)?.sync_all()?;
        Ok(())
    }
}
