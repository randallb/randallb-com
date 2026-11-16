//! Owned machine state, configuration checks, and bounded SmolVM control calls.

use super::{check_cancelled, valid_digest, Cli, Owner, CANCELLED, MACHINE_PREFIX};
use serde_json::{json, Value};
use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{atomic::Ordering, mpsc},
    thread,
    time::{Duration, Instant},
};

const MAX_CONTROL_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

pub(super) struct State {
    pub(super) directory: PathBuf,
    owner_path: PathBuf,
    _lock: File,
}

struct ControlOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl State {
    pub(super) fn acquire(home: &Path) -> Result<Self, Box<dyn Error>> {
        let directory = home.join(".cwr-smolvm-shell");
        if !directory.exists() {
            fs::create_dir(&directory)?;
        }
        let metadata = fs::symlink_metadata(&directory)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("SmolVM state path must be a real directory".into());
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(directory.join("lock"))?;
        // SAFETY: the file stays alive in State for the whole invocation.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("another SmolVM Shell action is already active".into());
        }
        Ok(Self {
            owner_path: directory.join("owner.json"),
            directory,
            _lock: lock,
        })
    }

    fn read_owner(&self) -> Result<Option<Owner>, Box<dyn Error>> {
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.owner_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        Read::by_ref(&mut file).take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err("SmolVM owner record is too large".into());
        }
        let owner: Owner = serde_json::from_slice(&bytes)?;
        validate_owner(&owner)?;
        Ok(Some(owner))
    }

    pub(super) fn write_owner(&self, owner: &Owner) -> Result<(), Box<dyn Error>> {
        validate_owner(owner)?;
        let temporary = self
            .directory
            .join(format!("owner-{}.tmp", uuid::Uuid::new_v4().simple()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        serde_json::to_writer(&mut file, owner)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &self.owner_path)?;
        sync_directory(&self.directory)?;
        Ok(())
    }

    fn clear_owner(&self) -> Result<(), Box<dyn Error>> {
        fs::remove_file(&self.owner_path)?;
        sync_directory(&self.directory)?;
        Ok(())
    }

    pub(super) fn recover(&self, cli: &Cli<'_>) -> Result<(), Box<dyn Error>> {
        if let Some(owner) = self.read_owner()? {
            if owner.smolvm != cli.executable {
                return Err(
                    "previous SmolVM executable differs; exact owned machine recovery is required"
                        .into(),
                );
            }
            self.cleanup(cli, &owner)?;
        }
        Ok(())
    }

    pub(super) fn cleanup(&self, cli: &Cli<'_>, owner: &Owner) -> Result<(), Box<dyn Error>> {
        if let Some(record) = machine_record(cli, &owner.machine)? {
            verify_record(&record, owner)?;
            // Stop can report an already stopped machine as an error. Delete is
            // still attempted; absence is the required outcome.
            let _ = run_control(
                cli,
                &["stop", "--name", &owner.machine, "--timeout", "0"],
                Duration::from_secs(8),
                false,
            );
            let _ = run_control(
                cli,
                &["delete", "--name", &owner.machine, "--force"],
                Duration::from_secs(8),
                false,
            );
            if machine_record(cli, &owner.machine)?.is_some() {
                return Err(format!(
                    "owned SmolVM machine {} remains after cleanup",
                    owner.machine
                )
                .into());
            }
        }
        self.clear_owner()?;
        Ok(())
    }
}

pub(super) fn validate_owner(owner: &Owner) -> Result<(), Box<dyn Error>> {
    let suffix = owner
        .machine
        .strip_prefix(MACHINE_PREFIX)
        .ok_or("invalid SmolVM owner name")?;
    if owner.schema != 1
        || suffix.len() != 32
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || !owner.smolvm.is_absolute()
        || !owner.image.is_absolute()
        || !valid_digest(&owner.image_sha256)
    {
        return Err("invalid SmolVM owner record".into());
    }
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

pub(super) fn machine_record(
    cli: &Cli<'_>,
    machine: &str,
) -> Result<Option<Value>, Box<dyn Error>> {
    let output = run_checked(cli, &["ls", "--json"], Duration::from_secs(4), false)?;
    let records: Vec<Value> = serde_json::from_slice(&output)?;
    Ok(records.into_iter().find(|record| record["name"] == machine))
}

pub(super) fn verify_record(record: &Value, owner: &Owner) -> Result<(), Box<dyn Error>> {
    // SmolVM imports local archives by content and reports the digest as its
    // image identity, not the archive path passed to `machine create`.
    let image_identity = format!("local:{}", owner.image_sha256);
    let expected = json!({
        "name":owner.machine, "image":image_identity, "network":false,
        "mounts":0, "ports":0, "cpus":1, "memory_mib":1024,
        "storage_gb":4, "overlay_gb":2,
    });
    for (key, value) in expected
        .as_object()
        .ok_or("invalid expected VM configuration")?
    {
        if &record[key] != value {
            return Err(format!(
                "owned SmolVM machine {key} differs; refusing execution or deletion"
            )
            .into());
        }
    }
    Ok(())
}

pub(super) fn run_checked(
    cli: &Cli<'_>,
    args: &[&str],
    timeout: Duration,
    cancel_sensitive: bool,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let output = run_control(cli, args, timeout, cancel_sensitive)?;
    if !output.status.success() {
        return Err(format!(
            "SmolVM machine {} failed: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output.stdout)
}

fn run_control(
    cli: &Cli<'_>,
    args: &[&str],
    timeout: Duration,
    cancel_sensitive: bool,
) -> Result<ControlOutput, Box<dyn Error>> {
    if cancel_sensitive {
        check_cancelled()?;
    }
    let mut child = Command::new(cli.executable)
        .arg("machine")
        .args(args)
        .current_dir(cli.state)
        .env_clear()
        .env("HOME", cli.home)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or("SmolVM control stdout is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("SmolVM control stderr is unavailable")?;
    let (out_tx, out_rx) = mpsc::channel();
    let (err_tx, err_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = out_tx.send(read_capped(stdout, MAX_CONTROL_OUTPUT_BYTES));
    });
    thread::spawn(move || {
        let _ = err_tx.send(read_capped(stderr, MAX_CONTROL_OUTPUT_BYTES));
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if cancel_sensitive && CANCELLED.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("SmolVM action was cancelled".into());
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("SmolVM machine {} timed out", args[0]).into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = out_rx.recv_timeout(Duration::from_secs(2))??;
    let stderr = err_rx.recv_timeout(Duration::from_secs(2))??;
    Ok(ControlOutput {
        status,
        stdout,
        stderr,
    })
}

fn read_capped(mut reader: impl Read, cap: usize) -> io::Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok(result);
        }
        if result.len().saturating_add(count) > cap {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SmolVM control output is too large",
            ));
        }
        result.extend_from_slice(&buffer[..count]);
    }
}
