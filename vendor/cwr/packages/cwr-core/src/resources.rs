//! Exact local file resources for CWR runs.

use std::{
    collections::BTreeMap,
    fs, io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::CwrResult;

const RESOURCE_REF_PREFIX: &str = "cwp://resources/sha256/";
const TREE_FORMAT: &str = "cwr.tree/1";

fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(io::Error::new(io::ErrorKind::InvalidInput, message.into()).into())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceReference {
    pub reference: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TreeManifest {
    format: String,
    entries: BTreeMap<String, ResourceReference>,
}

/// Local immutable storage for resource bytes.
#[derive(Debug, Clone)]
pub struct RunResourceStore {
    root: PathBuf,
}

impl RunResourceStore {
    pub fn open(root: impl AsRef<Path>) -> CwrResult<Self> {
        let store = Self {
            root: root.as_ref().to_path_buf(),
        };
        fs::create_dir_all(store.blobs_root())?;
        fs::set_permissions(&store.root, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(store.blobs_root(), fs::Permissions::from_mode(0o700))?;
        Ok(store)
    }

    fn blobs_root(&self) -> PathBuf {
        self.root.join("resources").join("sha256")
    }

    fn blob_path(&self, digest: &str) -> PathBuf {
        self.blobs_root().join(digest)
    }

    fn put_bytes(&self, bytes: &[u8]) -> CwrResult<ResourceReference> {
        let sha256 = digest(bytes);
        let target = self.blob_path(&sha256);
        if !target.exists() {
            let temporary = target.with_extension(format!(
                "{}.tmp",
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            ));
            fs::write(&temporary, bytes)?;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
            match fs::rename(&temporary, &target) {
                Ok(()) => {}
                Err(_error) if target.exists() => {
                    let _ = fs::remove_file(temporary);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(ResourceReference {
            reference: format!("{RESOURCE_REF_PREFIX}{sha256}"),
            sha256,
        })
    }

    fn get_bytes(&self, resource: &ResourceReference) -> CwrResult<Vec<u8>> {
        validate_resource_reference(resource)?;
        let bytes = fs::read(self.blob_path(&resource.sha256))?;
        if digest(&bytes) != resource.sha256 {
            return invalid("immutable resource bytes do not match their reference");
        }
        Ok(bytes)
    }

    fn put_json<T: Serialize>(&self, value: &T) -> CwrResult<ResourceReference> {
        self.put_bytes(&serde_json::to_vec(value)?)
    }
}

/// Store a complete directory as an immutable tree. Symlinks and non-UTF-8
/// names are rejected rather than silently widening or changing the tree.
pub fn put_directory_resource(
    store: &RunResourceStore,
    directory: impl AsRef<Path>,
) -> CwrResult<ResourceReference> {
    let directory = directory.as_ref();
    if !directory.is_dir() {
        return invalid("resource input must be a directory");
    }
    let mut entries = BTreeMap::new();
    collect_directory_resources(store, directory, Path::new(""), &mut entries)?;
    store.put_json(&TreeManifest {
        format: TREE_FORMAT.into(),
        entries,
    })
}

/// Materialize one immutable byte resource at a caller-selected destination.
pub fn materialize_resource(
    store: &RunResourceStore,
    resource: &ResourceReference,
    destination: impl AsRef<Path>,
) -> CwrResult<()> {
    let destination = destination.as_ref();
    if destination.exists() {
        return invalid("resource materialization destination already exists");
    }
    fs::write(destination, store.get_bytes(resource)?)?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o444))?;
    Ok(())
}

fn collect_directory_resources(
    store: &RunResourceStore,
    root: &Path,
    relative: &Path,
    entries: &mut BTreeMap<String, ResourceReference>,
) -> CwrResult<()> {
    let mut children = fs::read_dir(root.join(relative))?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let file_name = child.file_name().into_string().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "resource paths must be UTF-8")
        })?;
        let child_relative = relative.join(file_name);
        let metadata = fs::symlink_metadata(child.path())?;
        if metadata.file_type().is_symlink() {
            return invalid("resource trees cannot contain symlinks");
        }
        if metadata.is_dir() {
            collect_directory_resources(store, root, &child_relative, entries)?;
        } else if metadata.is_file() {
            let path = child_relative
                .to_str()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "resource paths must be UTF-8")
                })?
                .replace('\\', "/");
            entries.insert(path, store.put_bytes(&fs::read(child.path())?)?);
        } else {
            return invalid("resource trees can contain only regular files and directories");
        }
    }
    Ok(())
}

fn validate_resource_reference(resource: &ResourceReference) -> CwrResult<()> {
    if resource.sha256.len() != 64
        || !resource.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || resource.reference != format!("{RESOURCE_REF_PREFIX}{}", resource.sha256)
    {
        return invalid("resource reference is invalid");
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
