//! Private bootstrap metadata for the resident GraphQL endpoint.
use crate::CwrResult;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// This file is a local bearer credential. Never print it or include it in diagnostics.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalGraphqlEndpoint {
    pub version: u32,
    pub socket_path: PathBuf,
    pub runtime_ref: String,
    pub actor_ref: String,
    pub token: String,
    pub pid: u32,
}

impl LocalGraphqlEndpoint {
    pub fn read(path: &Path) -> CwrResult<Self> {
        let mut file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let metadata = file.metadata()?;
        // SAFETY: geteuid has no preconditions and only reads the process identity.
        let uid = unsafe { libc::geteuid() };
        if !metadata.is_file()
            || metadata.uid() != uid
            || metadata.mode() & 0o077 != 0
            || metadata.len() > 8192
        {
            return Err(std::io::Error::other(
                "GraphQL discovery must be a private regular file owned by this user",
            )
            .into());
        }
        let mut bytes = Vec::new();
        file.by_ref().take(8193).read_to_end(&mut bytes)?;
        if bytes.len() > 8192 {
            return Err(std::io::Error::other("GraphQL discovery is too large").into());
        }
        let endpoint: Self = serde_json::from_slice(&bytes)?;
        if endpoint.version != 2
            || !endpoint.socket_path.is_absolute()
            || endpoint.token.len() != 64
            || endpoint.actor_ref.is_empty()
        {
            return Err(std::io::Error::other("invalid local GraphQL endpoint").into());
        }
        let socket = fs::symlink_metadata(&endpoint.socket_path)?;
        if !socket.file_type().is_socket() || socket.uid() != uid || socket.mode() & 0o077 != 0 {
            return Err(std::io::Error::other(
                "GraphQL endpoint must name a private Unix socket owned by this user",
            )
            .into());
        }
        Ok(endpoint)
    }

    pub fn url(&self) -> String {
        "http://localhost/graphql".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn discovery_rejects_public_symlinked_and_invalid_socket_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("graphql.json");
        let socket_path = directory.path().join("graphql.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)).unwrap();
        let endpoint = LocalGraphqlEndpoint {
            version: 2,
            socket_path,
            runtime_ref: "cwp://example.test/runtime".into(),
            actor_ref: "cwp://example.test/actor".into(),
            token: "a".repeat(64),
            pid: std::process::id(),
        };
        fs::write(&path, serde_json::to_vec(&endpoint).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(LocalGraphqlEndpoint::read(&path).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(LocalGraphqlEndpoint::read(&path).is_ok());
        let link = directory.path().join("link.json");
        symlink(&path, &link).unwrap();
        assert!(LocalGraphqlEndpoint::read(&link).is_err());
        fs::set_permissions(&endpoint.socket_path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(LocalGraphqlEndpoint::read(&path).is_err());
        fs::set_permissions(&endpoint.socket_path, fs::Permissions::from_mode(0o600)).unwrap();
        let socket_link = directory.path().join("link.sock");
        symlink(&endpoint.socket_path, &socket_link).unwrap();
        let linked = LocalGraphqlEndpoint {
            socket_path: socket_link,
            ..endpoint.clone()
        };
        fs::write(&path, serde_json::to_vec(&linked).unwrap()).unwrap();
        assert!(LocalGraphqlEndpoint::read(&path).is_err());
        let old = serde_json::json!({"version":1,"address":"127.0.0.1:12345","runtime_ref":endpoint.runtime_ref,"actor_ref":endpoint.actor_ref,"token":endpoint.token,"pid":endpoint.pid});
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(LocalGraphqlEndpoint::read(&path).is_err());
        let remote = LocalGraphqlEndpoint {
            socket_path: directory.path().join("missing.sock"),
            ..endpoint
        };
        fs::write(&path, serde_json::to_vec(&remote).unwrap()).unwrap();
        assert!(LocalGraphqlEndpoint::read(&path).is_err());
    }
}
