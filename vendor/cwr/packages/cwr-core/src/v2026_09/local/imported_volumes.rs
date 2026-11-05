//! Local folder grants for volume requests declared by imported runtimes.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{v2026_09::graph, CwrResult};

use super::invalid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalVolumeBindingsFile {
    version: u32,
    bindings: Vec<LocalImportedVolume>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalImportedVolume {
    source: PathBuf,
    alias: String,
    path: PathBuf,
    access: String,
    #[serde(rename = "origin")]
    _origin: Option<BindingOrigin>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum BindingOrigin {
    Registry,
}

pub fn load(path: &Path) -> CwrResult<Vec<graph::ImportedVolumeBinding>> {
    let bindings_path = bindings_path(path)?;
    let contents = match fs::read_to_string(&bindings_path) {
        Ok(contents) => contents,
        // An unavailable state directory cannot contain saved grants. Inspection
        // remains read-only even when an ancestor is a regular file.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new())
        }
        Err(error) => return Err(error.into()),
    };
    let file: LocalVolumeBindingsFile = serde_json::from_str(&contents)?;
    if file.version != 1 {
        return invalid("unsupported imported volume bindings version");
    }
    file.bindings
        .into_iter()
        .map(|binding| -> CwrResult<_> {
            Ok(graph::ImportedVolumeBinding {
                source: fs::canonicalize(binding.source)?,
                alias: binding.alias,
                path: fs::canonicalize(binding.path)?,
                access: binding.access,
            })
        })
        .collect::<CwrResult<Vec<_>>>()
}

pub fn bindings_path(path: &Path) -> CwrResult<PathBuf> {
    let canonical = fs::canonicalize(path)?;
    let digest = Sha256::digest(canonical.as_os_str().as_encoded_bytes());
    Ok(
        crate::workspace_environment::product_state_directory_path()?
            .join("imported-volumes")
            .join(format!("{}.json", hex::encode(digest))),
    )
}
