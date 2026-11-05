//! Exclusive execution in a shared workspace, independent of a host's state store.

use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
    thread,
    time::Duration,
};

use crate::CwrResult;

/// Keeps a workspace available to one adapter attempt until the guard is dropped.
/// The operating system also releases the lease when its host exits.
pub struct WorkspaceExecutionLease {
    _file: File,
}

impl WorkspaceExecutionLease {
    /// Try once so a host can defer queued work without occupying a worker.
    pub fn try_acquire(workspace: &Path) -> CwrResult<Option<Self>> {
        let file = open_lock_file(workspace)?;
        Ok(try_lock(&file)?.then_some(Self { _file: file }))
    }

    /// Wait without claiming the invocation or holding the admission store lock.
    /// `runnable` lets a queued cancellation return without starting an adapter.
    pub fn acquire(
        workspace: &Path,
        mut runnable: impl FnMut() -> CwrResult<bool>,
    ) -> CwrResult<Option<Self>> {
        let file = open_lock_file(workspace)?;
        loop {
            if !runnable()? {
                return Ok(None);
            }
            if try_lock(&file)? {
                return Ok(Some(Self { _file: file }));
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

fn open_lock_file(workspace: &Path) -> CwrResult<File> {
    let directory = fs::canonicalize(workspace)?.join(".cwr");
    fs::create_dir_all(&directory)?;
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(directory.join("workspace-execution.lock"))?)
}

fn try_lock(file: &File) -> CwrResult<bool> {
    loop {
        // SAFETY: `file` owns the descriptor throughout this call. flock
        // neither takes ownership nor retains the descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        match io::Error::last_os_error() {
            error if error.kind() == io::ErrorKind::Interrupted => continue,
            error if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            error => return Err(error.into()),
        }
    }
}
