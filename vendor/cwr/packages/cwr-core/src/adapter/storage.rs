//! Per-user adapter storage, independent of workspace persistence.
use crate::CwrResult;
use std::{
    env, fs,
    io::{Read as _, Seek as _},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
};

/// Resolve an adapter directory without creating it, for read-only import previews.
pub fn adapter_data_directory_path(key: &str) -> CwrResult<PathBuf> {
    if key.is_empty() || !key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return Err("Invalid adapter storage key".into());
    }
    let root = match env::var_os("CWR_ADAPTER_DATA_ROOT") {
        Some(root) if !root.is_empty() => PathBuf::from(root),
        _ => dirs::data_local_dir()
            .ok_or("Cannot determine adapter storage directory")?
            .join("CWR")
            .join("adapters"),
    };
    if !root.is_absolute() {
        return Err("Adapter data root must be absolute".into());
    }
    Ok(root.join(key))
}

/// Allocate an adapter's stable directory. The adapter owns everything below it.
pub fn adapter_data_directory(key: &str) -> CwrResult<PathBuf> {
    let directory = adapter_data_directory_path(key)?;
    private_directory(
        directory
            .parent()
            .ok_or("Adapter directory has no parent")?,
    )?;
    private_directory(&directory)?;
    Ok(directory)
}

/// Validate and create a host-supplied private adapter directory.
pub fn prepare_adapter_data_directory(path: &Path) -> CwrResult<()> {
    if !path.is_absolute() {
        return Err("Adapter data directory must be absolute".into());
    }
    private_directory(path)
}

fn private_directory(path: &Path) -> CwrResult<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    // SAFETY: geteuid only reads the effective process identity.
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err("Adapter data directory must be privately owned".into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterConfiguration {
    pub data_directory: PathBuf,
    pub provider_home: PathBuf,
}

/// Ask the adapter to initialize its private storage and report its configuration.
pub fn configure_adapter(adapter_ref: &str) -> CwrResult<AdapterConfiguration> {
    if adapter_ref != super::CODEX_ADAPTER_REF {
        return Err("Adapter configuration is unavailable".into());
    }
    let data_directory = adapter_data_directory("codex")?;
    let executable =
        super::sibling_adapter_executable("CWR_CODEX_ADAPTER_EXECUTABLE", "cwr-adapter-codex")?;
    let mut output = tempfile::tempfile()?;
    let mut child = std::process::Command::new(executable)
        .arg("--configure")
        .env("CWR_ADAPTER_DATA_DIRECTORY", &data_directory)
        .stdin(std::process::Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline || output.metadata()?.len() > 16 * 1024 {
            super::terminate_probe_group(&mut child);
            return Err("Adapter configuration timed out or exceeded its output limit".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    if !status.success() || output.metadata()?.len() > 16 * 1024 {
        return Err("Adapter configuration failed".into());
    }
    output.rewind()?;
    let mut bytes = Vec::new();
    output.take(16 * 1024).read_to_end(&mut bytes)?;
    let configuration: AdapterConfiguration = serde_json::from_slice(&bytes)?;
    if configuration.data_directory != data_directory
        || !configuration.provider_home.starts_with(&data_directory)
        || configuration.provider_home == data_directory
        || configuration
            .provider_home
            .strip_prefix(&data_directory)?
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("Adapter returned an invalid provider directory".into());
    }
    let resolved_home = configuration.provider_home.canonicalize()?;
    let resolved_data = data_directory.canonicalize()?;
    if !resolved_home.starts_with(&resolved_data) || resolved_home == resolved_data {
        return Err("Adapter provider directory escapes its data directory".into());
    }
    Ok(configuration)
}

pub(super) fn shared_codex_home() -> CwrResult<PathBuf> {
    Ok(configure_adapter(super::CODEX_ADAPTER_REF)?.provider_home)
}
