use super::{config, policy};
use crate::CwrResult;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct SkippedImport {
    pub path: PathBuf,
    pub reason: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ImportReport {
    pub source_home: PathBuf,
    pub provider_home: PathBuf,
    pub dry_run: bool,
    /// Proposed imports in dry-run mode; completed file writes otherwise.
    pub imported: Vec<PathBuf>,
    pub skipped: Vec<SkippedImport>,
    pub conflicts: Vec<PathBuf>,
    pub backup_directory: Option<PathBuf>,
    pub reauthentication_required: bool,
    pub warnings: Vec<String>,
}

enum Content {
    Directory,
    File(PathBuf),
    Bytes(Vec<u8>),
}
struct Update {
    relative: PathBuf,
    content: Content,
    executable: bool,
    existing: bool,
}

pub(super) fn run(
    data: &Path,
    source: &Path,
    dry_run: bool,
    replace: bool,
) -> CwrResult<ImportReport> {
    validate_source(data, source)?;
    // Rebase imported paths against the spelling that the filesystem reports.
    // On case-insensitive macOS volumes, callers can reach an existing `cwr`
    // directory through a `CWR` path. Codex canonicalizes CODEX_HOME before it
    // validates local plugin marketplaces, so retaining the caller's spelling
    // here makes otherwise valid imported plugin paths look external.
    let unresolved_home = data.join("provider-home");
    // Reject a provider-home symlink before resolving aliases in its ancestors.
    // Otherwise canonicalization would hide a final symlink and imports could
    // overwrite files outside adapter storage, including the source home.
    policy::validate_destination(&unresolved_home)?;
    let home = resolved_path(&unresolved_home)?;
    policy::validate_destination(&home)?;
    let mut report = ImportReport {
        source_home: source.to_owned(), provider_home: home.clone(), dry_run,
        imported: Vec::new(), skipped: Vec::new(), conflicts: Vec::new(), backup_directory: None,
        reauthentication_required: true,
        warnings: vec!["File-backed credentials are imported; OS-keyring and encrypted secrets are not relocated. Check provider and MCP sign-in after import.".into(),
            "Multi-file imports are resumable merges, not atomic snapshots. Stop sessions using this home before replacing conflicts.".into()],
    };
    let mut updates = Vec::new();
    visit(
        source,
        &home,
        Path::new(""),
        replace,
        &mut report,
        &mut updates,
    )?;
    report.imported = updates
        .iter()
        .map(|update| update.relative.clone())
        .collect();
    if dry_run {
        return Ok(report);
    }
    ensure_directory(&home)?;
    if updates.iter().any(|update| update.existing) {
        let backups = data.join("reimport-backups");
        ensure_directory(&backups)?;
        let backup = backups.join(uuid::Uuid::new_v4().to_string());
        ensure_directory(&backup)?;
        for update in updates.iter().filter(|update| update.existing) {
            check_parents(&home, &update.relative)?;
            let destination = backup.join(&update.relative);
            ensure_directory(destination.parent().ok_or("Missing backup parent")?)?;
            copy_file(&home.join(&update.relative), &destination, false)?;
        }
        report.backup_directory = Some(backup);
    }
    for update in updates {
        check_parents(&home, &update.relative)?;
        let path = home.join(&update.relative);
        ensure_directory(path.parent().ok_or("Missing import parent")?)?;
        match update.content {
            Content::Directory => {
                ensure_directory(&path)?;
                continue;
            }
            Content::File(source) => copy_file(&source, &path, update.existing)?,
            Content::Bytes(bytes) => write_private(&path, &bytes, update.existing)?,
        }
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if update.executable { 0o700 } else { 0o600 }),
        )?;
    }
    Ok(report)
}

pub(super) fn validate_source(data: &Path, source: &Path) -> CwrResult<()> {
    if !source.is_absolute() || !source.is_dir() {
        return Err("Codex import source must be an existing absolute directory".into());
    }
    let canonical_source = source.canonicalize()?;
    let canonical_data = resolved_path(data)?;
    if canonical_data.starts_with(&canonical_source)
        || canonical_source.starts_with(&canonical_data)
    {
        return Err("Adapter storage must be outside the source home and cannot contain it".into());
    }
    Ok(())
}

fn visit(
    source_root: &Path,
    destination_root: &Path,
    relative: &Path,
    replace: bool,
    report: &mut ImportReport,
    updates: &mut Vec<Update>,
) -> CwrResult<()> {
    let source = source_root.join(relative);
    let metadata = if relative.as_os_str().is_empty() {
        fs::metadata(&source)?
    } else {
        fs::symlink_metadata(&source)?
    };
    if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
        report.skipped.push(SkippedImport {
            path: relative.to_owned(),
            reason: "symlink or special file".into(),
        });
        return Ok(());
    }
    if policy::excluded(relative) {
        report.skipped.push(SkippedImport {
            path: relative.to_owned(),
            reason: "runtime state or encrypted secrets".into(),
        });
        return Ok(());
    }
    let destination = destination_root.join(relative);
    policy::validate_destination(&destination)?;
    let existing = match fs::symlink_metadata(&destination) {
        Ok(existing) => Some(existing),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if existing
        .as_ref()
        .is_some_and(|entry| !entry.is_dir() && !entry.is_file())
    {
        return Err("Import destination contains a special file".into());
    }
    if existing
        .as_ref()
        .is_some_and(|existing| existing.is_dir() != metadata.is_dir())
    {
        report.conflicts.push(relative.to_owned());
        report.skipped.push(SkippedImport {
            path: relative.to_owned(),
            reason: "file/directory conflict; resolve manually".into(),
        });
        return Ok(());
    }
    if metadata.is_dir() {
        if !relative.as_os_str().is_empty() && existing.is_none() {
            updates.push(Update {
                relative: relative.to_owned(),
                content: Content::Directory,
                executable: false,
                existing: false,
            });
        }
        let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            visit(
                source_root,
                destination_root,
                &relative.join(entry.file_name()),
                replace,
                report,
                updates,
            )?;
        }
        return Ok(());
    }
    if relative == Path::new("config.toml") {
        let source_text = fs::read_to_string(&source)?;
        let destination_text = if existing.is_some() {
            Some(fs::read_to_string(&destination)?)
        } else {
            None
        };
        if let Some(bytes) = config::merge(
            &source_text,
            destination_text.as_deref(),
            source_root,
            destination_root,
            replace,
            report,
        )? {
            updates.push(Update {
                relative: relative.to_owned(),
                content: Content::Bytes(bytes),
                executable: false,
                existing: existing.is_some(),
            });
        }
        return Ok(());
    }
    if existing.is_some() {
        if identical(&source, &destination)? {
            report.skipped.push(SkippedImport {
                path: relative.to_owned(),
                reason: "unchanged".into(),
            });
            return Ok(());
        }
        report.conflicts.push(relative.to_owned());
        if !replace {
            return Ok(());
        }
    }
    updates.push(Update {
        relative: relative.to_owned(),
        content: Content::File(source),
        executable: metadata.permissions().mode() & 0o111 != 0,
        existing: existing.is_some(),
    });
    Ok(())
}

fn resolved_path(path: &Path) -> CwrResult<PathBuf> {
    if !path.is_absolute() {
        return Err("Adapter data directory must be absolute".into());
    }
    let mut ancestor = path;
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(
            ancestor
                .file_name()
                .ok_or("Invalid adapter data directory")?
                .to_owned(),
        );
        ancestor = ancestor.parent().ok_or("Invalid adapter data directory")?;
    }
    let mut result = ancestor.canonicalize()?;
    for name in suffix.into_iter().rev() {
        result.push(name);
    }
    Ok(result)
}

fn check_parents(home: &Path, relative: &Path) -> CwrResult<()> {
    policy::validate_destination(home)?;
    let mut path = home.to_owned();
    for component in relative.components() {
        path.push(component);
        policy::validate_destination(&path)?;
    }
    Ok(())
}

fn ensure_directory(path: &Path) -> CwrResult<()> {
    policy::validate_destination(path)?;
    if !path.exists() {
        if let Some(parent) = path.parent() {
            ensure_directory(parent)?;
        }
        fs::create_dir(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    if !path.is_dir() {
        return Err("Import parent must be a directory".into());
    }
    Ok(())
}

fn open_read(path: &Path) -> CwrResult<fs::File> {
    Ok(fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?)
}
fn identical(a: &Path, b: &Path) -> CwrResult<bool> {
    let mut a = open_read(a)?;
    let mut b = open_read(b)?;
    if a.metadata()?.len() != b.metadata()?.len() {
        return Ok(false);
    }
    let mut left = [0; 8192];
    let mut right = [0; 8192];
    loop {
        let n = a.read(&mut left)?;
        b.read_exact(&mut right[..n])?;
        if left[..n] != right[..n] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}
fn copy_file(source: &Path, destination: &Path, replace: bool) -> CwrResult<()> {
    let mut source = open_read(source)?;
    let mode = if source.metadata()?.permissions().mode() & 0o111 != 0 {
        0o700
    } else {
        0o600
    };
    let mut temporary = temporary(destination)?;
    std::io::copy(&mut source, &mut temporary)?;
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(mode))?;
    persist(temporary, destination, replace)
}
pub(super) fn write_private(path: &Path, bytes: &[u8], replace: bool) -> CwrResult<()> {
    let mut temporary = temporary(path)?;
    temporary.write_all(bytes)?;
    persist(temporary, path, replace)
}
fn temporary(path: &Path) -> CwrResult<tempfile::NamedTempFile> {
    policy::validate_destination(path)?;
    Ok(tempfile::NamedTempFile::new_in(
        path.parent().ok_or("Import destination has no parent")?,
    )?)
}
fn persist(temporary: tempfile::NamedTempFile, path: &Path, replace: bool) -> CwrResult<()> {
    policy::validate_destination(path)?;
    temporary.as_file().sync_all()?;
    if replace {
        temporary.persist(path)?;
    } else {
        temporary.persist_noclobber(path)?;
    }
    Ok(())
}
