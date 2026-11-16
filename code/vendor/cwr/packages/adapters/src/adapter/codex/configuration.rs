//! Portable Codex-home import, shared by initialization and explicit reimport.
mod bundled;
mod config;
mod importer;
mod policy;
#[cfg(test)]
mod tests;

use crate::CwrResult;
use cwr::adapter::{adapter_data_directory, prepare_adapter_data_directory, AdapterConfiguration};
pub use importer::{ImportReport, SkippedImport};
use std::{
    env, fs,
    fs::OpenOptions,
    os::fd::AsRawFd as _,
    path::{Path, PathBuf},
};
const MIGRATION: &str = "portable-import-v2.json";

pub fn configure() -> CwrResult<AdapterConfiguration> {
    let data = data_directory(false)?;
    let source = if data.join(MIGRATION).exists() {
        None
    } else {
        source_home()?
    };
    configure_at(data, source)
}

fn configure_at(data: PathBuf, source: Option<PathBuf>) -> CwrResult<AdapterConfiguration> {
    let _lock = lock(&data, false)?;
    let home = data.join("provider-home");
    policy::validate_destination(&home)?;
    if !data.join(MIGRATION).exists() {
        let source = source.filter(|source| source.exists());
        if let Some(source) = source {
            let report = importer::run(&data, &source, false, false)?;
            importer::write_private(&data.join(MIGRATION), &serde_json::to_vec(&report)?, false)?;
        } else if !home.exists() {
            prepare_adapter_data_directory(&home)?;
        }
    }
    prepare_adapter_data_directory(&home)?;
    bundled::repair(&home)?;
    Ok(AdapterConfiguration {
        data_directory: data,
        provider_home: home,
    })
}

/// Reimport portable content. A dry run does not allocate storage or modify permissions.
pub fn import_home(source: PathBuf, dry_run: bool, replace: bool) -> CwrResult<ImportReport> {
    let data = data_directory(true)?;
    importer::validate_source(&data, &source)?;
    if !dry_run {
        prepare_adapter_data_directory(&data)?;
    }
    let _lock = lock(&data, dry_run)?;
    let report = importer::run(&data, &source, dry_run, replace)?;
    if !dry_run {
        importer::write_private(&data.join(MIGRATION), &serde_json::to_vec(&report)?, true)?;
    }
    Ok(report)
}

fn data_directory(dry_run: bool) -> CwrResult<PathBuf> {
    match env::var_os("CWR_ADAPTER_DATA_DIRECTORY") {
        Some(path) => {
            let path = PathBuf::from(path);
            policy::validate_destination(&path)?;
            if !dry_run {
                prepare_adapter_data_directory(&path)?;
            }
            Ok(path)
        }
        None if dry_run => cwr::adapter::adapter_data_directory_path("codex"),
        None => adapter_data_directory("codex"),
    }
}

fn lock(data: &Path, dry_run: bool) -> CwrResult<Option<fs::File>> {
    policy::validate_destination(data)?;
    let path = data.join("configure.lock");
    policy::validate_destination(&path)?;
    if fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_file()) {
        return Err("Adapter configuration lock must be a regular file".into());
    }
    let lock = if dry_run {
        match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
    } else {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        // SAFETY: the descriptor remains owned by lock for the whole guarded import.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock {
            return Err(error.into());
        }
        if std::time::Instant::now() >= deadline {
            return Err("Adapter configuration lock timed out".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Ok(Some(lock))
}

fn source_home() -> CwrResult<Option<PathBuf>> {
    let source = env::var_os("CWR_CODEX_SOURCE_HOME")
        .or_else(|| env::var_os("CODEX_HOME"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")));
    if source.as_ref().is_some_and(|source| !source.is_absolute()) {
        return Err("Codex source home must be absolute".into());
    }
    Ok(source)
}
