use std::{error::Error, ffi::OsStr};

use crate::{host::run_sibling_host, workspace::selected_workspace_manifest};

pub(super) fn run_tui(
    arguments: &[std::ffi::OsString],
    workspace: Option<&OsStr>,
    actor_ref: Option<&OsStr>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let workspace = selected_workspace_manifest(workspace)?;
    let mut forwarded = Vec::new();
    if let Some(actor_ref) = actor_ref {
        forwarded.push("--actor".into());
        forwarded.push(actor_ref.into());
    }
    forwarded.extend_from_slice(arguments);
    let workspace = workspace
        .parent()
        .ok_or("CWR.toml has no containing workspace directory")?;
    run_sibling_host("cwr-tui", &forwarded, workspace)
}
