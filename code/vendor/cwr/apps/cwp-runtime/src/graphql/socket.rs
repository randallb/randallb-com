//! Private Unix listener with identity-checked cleanup and stale socket recovery.
use cwr::CwrResult;
use std::{
    fs, io,
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

pub(super) struct SocketFile {
    path: PathBuf,
    device: u64,
    inode: u64,
    _lock: fs::File,
}
impl SocketFile {
    pub fn bind(path: PathBuf) -> CwrResult<(UnixListener, Self)> {
        // Keep this inode on disk after release. Unlinking a lock file lets
        // concurrent starters lock different inodes for the same endpoint.
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path.with_extension("sock.lock"))?;
        let metadata = lock.metadata()?;
        // SAFETY: geteuid only reads the effective process identity.
        let uid = unsafe { libc::geteuid() };
        if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err(
                io::Error::other("GraphQL endpoint lock must be a private owned file").into(),
            );
        }
        // SAFETY: lock owns a valid file descriptor throughout this call.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "GraphQL runtime directory already has an endpoint owner",
                )
                .into());
            }
            return Err(error.into());
        }
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            // Never remove a regular file, symlink, foreign socket, or live listener.
            // SAFETY: geteuid only reads the effective process identity.
            let uid = unsafe { libc::geteuid() };
            if !metadata.file_type().is_socket() || metadata.uid() != uid {
                return Err(io::Error::other("GraphQL socket path is not an owned socket").into());
            }
            match UnixStream::connect(&path) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "GraphQL socket is already listening",
                    )
                    .into())
                }
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(&path)?,
                Err(e) => return Err(e.into()),
            }
        }
        let listener = UnixListener::bind(&path)?;
        let metadata = fs::symlink_metadata(&path)?;
        let cleanup = Self {
            path: path.clone(),
            device: metadata.dev(),
            inode: metadata.ino(),
            _lock: lock,
        };
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok((listener, cleanup))
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for SocketFile {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|m| {
            m.file_type().is_socket() && m.dev() == self.device && m.ino() == self.inode
        }) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn socket_lifecycle_recovers_stale_paths_without_replacing_live_or_unrelated_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("graphql.sock");
        let (listener, cleanup) = SocketFile::bind(path.clone()).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert!(SocketFile::bind(path.clone()).is_err());
        drop(listener);
        assert!(
            SocketFile::bind(path.clone()).is_err(),
            "ownership must outlive the listener"
        );
        drop(cleanup);
        assert!(!path.exists());
        // A process crash releases flock while leaving a stale socket entry.
        drop(UnixListener::bind(&path).unwrap());
        let (replacement, replacement_cleanup) = SocketFile::bind(path.clone()).unwrap();
        drop(replacement);
        drop(replacement_cleanup);
        assert!(!path.exists());
        fs::write(&path, "preserve me").unwrap();
        assert!(SocketFile::bind(path.clone()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "preserve me");
        fs::remove_file(&path).unwrap();
        let target = directory.path().join("target");
        fs::write(&target, "preserve me too").unwrap();
        symlink(&target, &path).unwrap();
        assert!(SocketFile::bind(path).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "preserve me too");
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    #[test]
    fn concurrent_starters_cannot_replace_the_winners_socket() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("graphql.sock");
        drop(UnixListener::bind(&path).unwrap());
        let barrier = Arc::new(Barrier::new(2));
        let (tx, rx) = std::sync::mpsc::channel();
        let workers = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                let path = path.clone();
                let tx = tx.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let result = SocketFile::bind(path.clone());
                    tx.send(result.is_ok()).unwrap();
                    barrier.wait();
                    if let Ok((_listener, _owner)) = result {
                        assert!(UnixStream::connect(path).is_ok());
                        barrier.wait();
                    } else {
                        barrier.wait();
                    }
                })
            })
            .collect::<Vec<_>>();
        drop(tx);
        let results = rx.into_iter().collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(results.iter().filter(|&&won| won).count(), 1);
        assert!(!path.exists());
    }
}
