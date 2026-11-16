use std::{env, error::Error, ffi::OsStr, path::PathBuf};

use cwr::v2026_09::workspace::{discover_workspace_manifest, resolve_workspace_manifest};

pub(super) const CWR_WORKSPACE_ENV: &str = "CWR_WORKSPACE";

pub(super) fn selected_workspace_manifest(
    requested: Option<&OsStr>,
) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    if let Some(path) = requested {
        if path.is_empty() {
            return Err("--workspace must not be empty".into());
        }
        return resolve_workspace_manifest(PathBuf::from(path));
    }
    if let Some(path) = env::var_os(CWR_WORKSPACE_ENV).filter(|path| !path.is_empty()) {
        return resolve_workspace_manifest(PathBuf::from(path));
    }
    discover_workspace_manifest(env::current_dir()?)
}
