use std::{
    fs::{self, OpenOptions},
    io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use cwr::v2026_09::local::{connect_local_resident, stop_local_resident_if_pid};
use serde::{Deserialize, Serialize};

const MARKER_NAME: &str = "resident-environment.json";
const REQUIREMENT_NAME: &str = "required-resident-environment.json";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResidentEnvironment {
    pid: u32,
    codex_executable: Option<String>,
    #[serde(default)]
    smolvm: SmolvmEnvironment,
    #[serde(default)]
    sms_bridge_socket: Option<String>,
    #[serde(default)]
    codex_service_tier_file: Option<String>,
    #[serde(default)]
    analytics_environment: Option<String>,
    #[serde(default)]
    adapter_storage: Option<AdapterStorageEnvironment>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RequiredResidentEnvironment {
    codex_executable: Option<String>,
    #[serde(default)]
    smolvm: SmolvmEnvironment,
    #[serde(default)]
    sms_bridge_socket: Option<String>,
    #[serde(default)]
    codex_service_tier_file: Option<String>,
    #[serde(default)]
    analytics_environment: Option<String>,
    #[serde(default)]
    adapter_storage: Option<AdapterStorageEnvironment>,
    expires_at_unix_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct AdapterStorageEnvironment {
    data_root: Option<String>,
    source_home: Option<String>,
    codex_home: Option<String>,
    user_home: Option<String>,
    xdg_data_home: Option<String>,
}

impl AdapterStorageEnvironment {
    fn configured() -> Self {
        // Keep explicit empty values too: import precedence distinguishes an
        // empty source override from an absent one. HOME/XDG select defaults.
        Self {
            data_root: std::env::var("CWR_ADAPTER_DATA_ROOT").ok(),
            source_home: std::env::var("CWR_CODEX_SOURCE_HOME").ok(),
            codex_home: std::env::var("CODEX_HOME").ok(),
            user_home: std::env::var("HOME").ok(),
            xdg_data_home: std::env::var("XDG_DATA_HOME").ok(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SmolvmEnvironment {
    executable: Option<String>,
    image: Option<String>,
    image_sha256: Option<String>,
}

impl SmolvmEnvironment {
    fn configured() -> Self {
        fn nonempty(name: &str) -> Option<String> {
            std::env::var(name).ok().filter(|value| !value.is_empty())
        }
        Self {
            executable: nonempty("CWR_SMOLVM_EXECUTABLE"),
            image: nonempty("CWR_SMOLVM_IMAGE"),
            image_sha256: nonempty("CWR_SMOLVM_IMAGE_SHA256"),
        }
    }
}

pub(crate) struct ResidentEnvironmentMarker {
    path: PathBuf,
    pid: u32,
}

impl ResidentEnvironmentMarker {
    pub(crate) fn publish(runtime_directory: &Path) -> cwr::CwrResult<Self> {
        let path = runtime_directory.join(MARKER_NAME);
        let marker = ResidentEnvironment {
            pid: std::process::id(),
            codex_executable: std::env::var("CWR_CODEX_EXECUTABLE").ok(),
            smolvm: SmolvmEnvironment::configured(),
            sms_bridge_socket: configured_sms_bridge_socket(),
            codex_service_tier_file: std::env::var("CWR_CODEX_SERVICE_TIER_FILE").ok(),
            analytics_environment: Some(cwr::posthog_environment_fingerprint()),
            adapter_storage: Some(AdapterStorageEnvironment::configured()),
        };
        validate_requirement(
            runtime_directory,
            &marker.codex_executable,
            &marker.smolvm,
            &marker.sms_bridge_socket,
            &marker.codex_service_tier_file,
        )?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        serde_json::to_writer(&mut file, &marker)?;
        std::io::Write::write_all(&mut file, b"\n")?;
        file.sync_all()?;
        Ok(Self {
            path,
            pid: marker.pid,
        })
    }

    pub(crate) fn validate_requirement(&self, runtime_directory: &Path) -> cwr::CwrResult<()> {
        let marker = read_marker(&self.path)?
            .ok_or_else(|| io::Error::other("resident environment marker disappeared"))?;
        validate_requirement(
            runtime_directory,
            &marker.codex_executable,
            &marker.smolvm,
            &marker.sms_bridge_socket,
            &marker.codex_service_tier_file,
        )?;
        remove_requirement(runtime_directory);
        Ok(())
    }
}

impl Drop for ResidentEnvironmentMarker {
    fn drop(&mut self) {
        let owned = fs::read(&self.path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<ResidentEnvironment>(&contents).ok())
            .is_some_and(|marker| marker.pid == self.pid);
        if owned {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn ensure(
    runtime_directory: &Path,
    runtime_socket: &Path,
    host_socket: &Path,
) -> cwr::CwrResult<()> {
    // A new configured runtime has no resident to reconcile. Its first startup
    // creates private directories and inherits the host's supplied environment.
    if !runtime_directory.try_exists()? {
        return Ok(());
    }
    let desired_codex = std::env::var("CWR_CODEX_EXECUTABLE").ok();
    let desired_smolvm = SmolvmEnvironment::configured();
    let desired_sms = configured_sms_bridge_socket();
    let desired_tier_file = std::env::var("CWR_CODEX_SERVICE_TIER_FILE").ok();
    let desired_storage = AdapterStorageEnvironment::configured();
    publish_requirement(
        runtime_directory,
        desired_codex.clone(),
        desired_smolvm.clone(),
        desired_sms.clone(),
        desired_tier_file.clone(),
    )?;
    let host = match connect_local_resident(host_socket) {
        Ok(host) => host,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(())
        }
        Err(error) => return Err(error.into()),
    };
    drop(host);

    let pid = fs::read_to_string(runtime_directory.join("resident-runtime.pid"))?
        .trim()
        .parse::<i32>()?;
    let current = read_marker(&runtime_directory.join(MARKER_NAME))?;
    if current.as_ref().is_some_and(|marker| {
        marker.pid == pid as u32
            && marker.codex_executable == desired_codex
            && marker.smolvm == desired_smolvm
            && marker.sms_bridge_socket == desired_sms
            && marker.codex_service_tier_file == desired_tier_file
            && marker.adapter_storage.as_ref() == Some(&desired_storage)
            && marker.analytics_environment.as_deref()
                == Some(cwr::posthog_environment_fingerprint().as_str())
    }) {
        remove_requirement(runtime_directory);
        return Ok(());
    }

    let outcome = (|| {
        let mut remaining = crate::drain_client::request(runtime_socket, "begin")?;
        let deadline = Instant::now() + Duration::from_secs(300);
        while remaining != 0 {
            if Instant::now() >= deadline {
                return Err(io::Error::other(format!(
                    "timed out waiting for {remaining} active tasks before updating the Workloop runtime environment",
                ))
                .into());
            }
            thread::sleep(Duration::from_millis(100));
            remaining = crate::drain_client::request(runtime_socket, "status")?;
        }
        crate::drain_client::request(runtime_socket, "finish")?;
        stop_local_resident_if_pid(
            &runtime_directory.join("resident-runtime.pid"),
            host_socket,
            pid,
        )
    })();
    if outcome.is_err() {
        let _ = crate::drain_client::request(runtime_socket, "cancel");
    }
    outcome
}

fn publish_requirement(
    runtime_directory: &Path,
    codex_executable: Option<String>,
    smolvm: SmolvmEnvironment,
    sms_bridge_socket: Option<String>,
    codex_service_tier_file: Option<String>,
) -> cwr::CwrResult<()> {
    let path = runtime_directory.join(REQUIREMENT_NAME);
    let temporary = runtime_directory.join(format!(
        ".required-resident-environment.{}.tmp",
        uuid::Uuid::new_v4(),
    ));
    let requirement = RequiredResidentEnvironment {
        codex_executable,
        smolvm,
        sms_bridge_socket,
        codex_service_tier_file,
        analytics_environment: Some(cwr::posthog_environment_fingerprint()),
        adapter_storage: Some(AdapterStorageEnvironment::configured()),
        expires_at_unix_ms: now_unix_ms().saturating_add(60_000),
    };
    let result = (|| -> cwr::CwrResult<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        serde_json::to_writer(&mut file, &requirement)?;
        std::io::Write::write_all(&mut file, b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn validate_requirement(
    runtime_directory: &Path,
    codex_executable: &Option<String>,
    smolvm: &SmolvmEnvironment,
    sms_bridge_socket: &Option<String>,
    codex_service_tier_file: &Option<String>,
) -> cwr::CwrResult<()> {
    let path = runtime_directory.join(REQUIREMENT_NAME);
    let requirement = match fs::read(&path) {
        Ok(contents) => serde_json::from_slice::<RequiredResidentEnvironment>(&contents).ok(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let Some(requirement) = requirement else {
        return Ok(());
    };
    if requirement.expires_at_unix_ms < now_unix_ms() {
        remove_requirement(runtime_directory);
        return Ok(());
    }
    if &requirement.codex_executable != codex_executable
        || &requirement.smolvm != smolvm
        || &requirement.sms_bridge_socket != sms_bridge_socket
        || &requirement.codex_service_tier_file != codex_service_tier_file
        || requirement.analytics_environment.as_deref()
            != Some(cwr::posthog_environment_fingerprint().as_str())
        || requirement
            .adapter_storage
            .as_ref()
            .is_some_and(|required| required != &AdapterStorageEnvironment::configured())
    {
        return Err(io::Error::other(
            "resident startup does not match Workloop's required runtime environment",
        )
        .into());
    }
    Ok(())
}

fn read_marker(path: &Path) -> cwr::CwrResult<Option<ResidentEnvironment>> {
    match fs::read(path) {
        Ok(contents) => Ok(serde_json::from_slice(&contents).ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn remove_requirement(runtime_directory: &Path) {
    let _ = fs::remove_file(runtime_directory.join(REQUIREMENT_NAME));
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn configured_sms_bridge_socket() -> Option<String> {
    nonempty_sms_bridge_socket(std::env::var("WORKLOOP_SMS_BRIDGE_SOCKET").ok())
}

fn nonempty_sms_bridge_socket(path: Option<String>) -> Option<String> {
    path.filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
#[path = "resident_environment_tests.rs"]
mod tests;
