//! Short, cancellable coordination shared by supported local processes.
use crate::CwrResult;
use std::{
    fs::{File, OpenOptions},
    io,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn acquire(path: &Path, mut cancelled: impl FnMut() -> bool) -> CwrResult<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if cancelled() {
            return Err("Input operation cancelled.".into());
        }
        // SAFETY: file owns this descriptor; flock does not retain it.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(file);
        }
        let error = io::Error::last_os_error();
        if !matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
        ) {
            return Err(error.into());
        }
        if Instant::now() >= deadline {
            return Err("Input settings are busy. Try again.".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
