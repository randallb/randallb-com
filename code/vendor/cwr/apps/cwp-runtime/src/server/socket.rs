use std::{
    fs, io,
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

use cwr::CwrResult;

#[derive(Clone, Copy, PartialEq, Eq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

pub(crate) struct OwnedSocket {
    path: PathBuf,
    identity: SocketIdentity,
}

impl OwnedSocket {
    pub(crate) fn bind(path: &Path) -> CwrResult<(Self, UnixListener)> {
        prepare(path)?;
        let listener = UnixListener::bind(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        let identity = identity(path)?.ok_or_else(|| {
            io::Error::other(format!(
                "CWP runtime path {} is not a Unix socket after binding",
                path.display()
            ))
        })?;
        Ok((
            Self {
                path: path.into(),
                identity,
            },
            listener,
        ))
    }
}

fn prepare(path: &Path) -> CwrResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let Some(existing_identity) = identity(path)? else {
        if path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "CWP runtime socket path {} is occupied by a non-socket file",
                    path.display()
                ),
            )
            .into());
        }
        return Ok(());
    };
    match UnixStream::connect(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("CWP runtime socket {} is already active", path.display()),
        )
        .into()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            remove_if_owned(path, existing_identity)?;
            Ok(())
        }
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!(
                "could not inspect CWP runtime socket {}: {error}",
                path.display()
            ),
        )
        .into()),
    }
}

fn identity(path: &Path) -> io::Result<Option<SocketIdentity>> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => Ok(Some(SocketIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn remove_if_owned(path: &Path, expected: SocketIdentity) -> io::Result<()> {
    if identity(path)? == Some(expected) {
        fs::remove_file(path)?;
    }
    Ok(())
}

impl Drop for OwnedSocket {
    fn drop(&mut self) {
        if let Err(error) = remove_if_owned(&self.path, self.identity) {
            eprintln!(
                "cwp-runtime could not remove its socket {}: {error}",
                self.path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_socket_cleanup_preserves_a_replacement_path() {
        let temporary = tempfile::tempdir().expect("temporary socket directory");
        let path = temporary.path().join("runtime.sock");
        let listener = UnixListener::bind(&path).expect("bind owned socket");
        let original = identity(&path)
            .expect("inspect owned socket")
            .expect("socket identity");
        drop(listener);
        fs::remove_file(&path).expect("remove original socket path");
        fs::write(&path, "replacement").expect("write replacement path");

        remove_if_owned(&path, original).expect("guarded socket cleanup");

        assert_eq!(fs::read_to_string(path).unwrap(), "replacement");
    }

    #[test]
    fn bound_socket_is_owner_only() {
        let temporary = tempfile::tempdir().expect("temporary socket directory");
        let path = temporary.path().join("runtime.sock");

        let (_socket, _listener) = OwnedSocket::bind(&path).expect("bind owned socket");

        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
