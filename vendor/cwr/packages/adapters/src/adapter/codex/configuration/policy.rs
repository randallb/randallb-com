use crate::CwrResult;
use std::{
    fs,
    path::{Component, Path},
};

/// Reject destination symlinks, including dangling links. The importer checks
/// each directory before descending, and rechecks target parents before writing.
/// Source roots may be aliases, but individual imported entries are never followed.
pub(super) fn validate_destination(path: &Path) -> CwrResult<()> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("Adapter import paths must be absolute and contain no parent traversal".into());
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err("Adapter import destination cannot contain symlinks".into())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub(super) fn excluded(relative: &Path) -> bool {
    let parts: Vec<_> = relative.iter().filter_map(|p| p.to_str()).collect();
    let Some(first) = parts.first().copied() else {
        return false;
    };
    if first == ".tmp" {
        return parts.len() > 1
            && !matches!(
                parts[1],
                "marketplaces" | "plugins" | "bundled-marketplaces"
            );
    }
    // Plugin assets can legitimately include databases, lockfiles and directories
    // called cache/log. Do not apply home-runtime exclusions inside their payloads.
    if matches!(first, "plugins") {
        return false;
    }
    if matches!(
        first,
        "log"
            | "logs"
            | "shell_snapshots"
            | "tmp"
            | "otel"
            | "feedback"
            | ".git"
            | "backups"
            | "reimport-backups"
            | "secrets"
    ) {
        return true;
    }
    let name = parts.last().copied().unwrap_or(first);
    if parts.len() == 1 || matches!(first, "sessions" | "archived_sessions") {
        return name.contains(".sqlite")
            || name.ends_with(".db")
            || name.ends_with(".db-wal")
            || name.ends_with(".db-shm")
            || name.ends_with(".lock")
            || name.ends_with(".sock")
            || matches!(
                name,
                "history.jsonl"
                    | "session_index.jsonl"
                    | "models_cache.json"
                    | "version.json"
                    | "portable-import-v2.json"
                    | ".codex-global-state.json"
            );
    }
    false
}
