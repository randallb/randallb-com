//! Host-local saved credentials.
//!
//! Existing saved API keys keep their private `.env` store until a person moves
//! a workspace through the new setup. New workspace secrets use the native
//! credential store through [`NativeSecretStore`].

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub struct SavedApiKey {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialRequest {
    List {},
    Save {
        name: String,
        value: String,
        id: Option<String>,
    },
    Remove {
        id: String,
    },
}

impl CredentialRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::List {} => Ok(()),
            Self::Save { name, value, id } => {
                if name.trim().is_empty()
                    || name.chars().count() > 80
                    || name.chars().any(char::is_control)
                {
                    return Err("API key name must contain 1–80 printable characters.");
                }
                if value.trim().is_empty() {
                    return Err("API key must contain 1–16384 bytes and no null characters.");
                }
                validate_secret(value)?;
                if let Some(id) = id {
                    validate_id(id)?;
                }
                Ok(())
            }
            Self::Remove { id } => validate_id(id),
        }
    }
}

#[cfg(test)]
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialMetadata {
    #[serde(default)]
    names: BTreeMap<String, String>,
}

pub(crate) trait SecretStore {
    fn put(&self, id: &str, value: &str) -> Result<(), &'static str>;
    fn get(&self, id: &str) -> Result<Option<String>, &'static str>;
    fn contains(&self, id: &str) -> Result<bool, &'static str>;
    fn remove(&self, id: &str) -> Result<(), &'static str>;
}

pub(crate) struct NativeSecretStore {
    service: String,
}

impl NativeSecretStore {
    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn current() -> Result<Self, &'static str> {
        let root = credentials_directory()?;
        Ok(Self::for_namespace(&root))
    }

    pub(crate) fn for_namespace(root: &Path) -> Self {
        let identity = hex::encode(Sha256::digest(root.as_os_str().as_encoded_bytes()));
        Self {
            service: format!("com.getworkloop.cwr.credentials.{}", &identity[..32]),
        }
    }
}

#[derive(Default)]
struct StoredKeys {
    values: BTreeMap<String, String>,
    names: BTreeMap<String, String>,
}

impl SecretStore for NativeSecretStore {
    fn put(&self, id: &str, value: &str) -> Result<(), &'static str> {
        validate_id(id)?;
        validate_secret(value)?;
        native_put(&self.service, id, value)
    }

    fn get(&self, id: &str) -> Result<Option<String>, &'static str> {
        validate_id(id)?;
        native_get(&self.service, id)
    }

    fn contains(&self, id: &str) -> Result<bool, &'static str> {
        validate_id(id)?;
        native_contains(&self.service, id)
    }

    fn remove(&self, id: &str) -> Result<(), &'static str> {
        validate_id(id)?;
        native_remove(&self.service, id)
    }
}

pub(crate) fn validate_secret(value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.len() > 16384 || value.contains('\0') {
        return Err("API key must contain 1–16384 bytes and no null characters.");
    }
    Ok(())
}

pub(crate) fn validate_id(id: &str) -> Result<(), &'static str> {
    uuid::Uuid::parse_str(id)
        .is_ok_and(|value| value.to_string() == id)
        .then_some(())
        .ok_or("Invalid saved API key identifier.")
}

fn credentials_directory() -> Result<PathBuf, &'static str> {
    let state = env::var_os("XDG_STATE_HOME")
        .filter(|value| Path::new(value).is_absolute())
        .map(PathBuf::from)
        .or_else(dirs::state_dir)
        .or_else(dirs::data_local_dir)
        .ok_or("Could not determine the local credential directory.")?;
    #[cfg(test)]
    let state = env::var_os("CWR_TEST_LEGACY_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or(state);
    let directory = state
        .join(if cfg!(any(target_os = "macos", target_os = "windows")) {
            "CWR"
        } else {
            "cwr"
        })
        .join("credentials");
    fs::create_dir_all(&directory)
        .map_err(|_| "Could not create the local credential directory.")?;
    #[cfg(unix)]
    fs::set_permissions(
        &directory,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .map_err(|_| "Could not secure the local credential directory.")?;
    Ok(directory)
}

fn credentials_path() -> Result<PathBuf, &'static str> {
    Ok(credentials_directory()?.join(".env"))
}

fn env_name(prefix: &str, id: &str) -> String {
    format!("{prefix}_{}", id.replace('-', "").to_ascii_uppercase())
}

fn id_from_env_name(name: &str, prefix: &str) -> Option<String> {
    let compact = name.strip_prefix(prefix)?;
    if compact.len() != 32 || !compact.chars().all(|value| value.is_ascii_hexdigit()) {
        return None;
    }
    uuid::Uuid::parse_str(compact)
        .ok()
        .map(|value| value.to_string())
}

fn read_legacy(path: &Path) -> Result<StoredKeys, &'static str> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(_) => return Err("Could not read the local credential .env file."),
    };
    let mut stored = StoredKeys::default();
    for line in text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let Some((name, encoded)) = line.split_once('=') else {
            return Err("Could not read the local credential .env file.");
        };
        let value: String = serde_json::from_str(encoded)
            .map_err(|_| "Could not read the local credential .env file.")?;
        if let Some(id) = id_from_env_name(name, "CWR_API_KEY_NAME_") {
            stored.names.insert(id, value);
        } else if let Some(id) = id_from_env_name(name, "CWR_API_KEY_VALUE_") {
            stored.values.insert(id, value);
        }
    }
    Ok(stored)
}

fn list_legacy(stored: &StoredKeys) -> Vec<SavedApiKey> {
    let mut keys: Vec<_> = stored
        .values
        .keys()
        .filter_map(|id| {
            stored.names.get(id).map(|name| SavedApiKey {
                id: id.clone(),
                name: name.clone(),
            })
        })
        .collect();
    keys.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    keys
}

fn write_legacy(path: &Path, stored: &StoredKeys) -> Result<(), &'static str> {
    let mut text = String::from(
        "# Workloop local credentials. Do not add this file to a shared loop folder.\n",
    );
    for (id, value) in &stored.values {
        let name = stored
            .names
            .get(id)
            .ok_or("Could not write the local credential .env file.")?;
        text.push_str(&format!(
            "{}={}\n",
            env_name("CWR_API_KEY_NAME", id),
            serde_json::to_string(name)
                .map_err(|_| "Could not write the local credential .env file.")?
        ));
        text.push_str(&format!(
            "{}={}\n",
            env_name("CWR_API_KEY_VALUE", id),
            serde_json::to_string(value)
                .map_err(|_| "Could not write the local credential .env file.")?
        ));
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| "Could not write the local credential .env file.")?;
    use std::io::Write;
    let result = (|| -> Result<(), &'static str> {
        file.write_all(text.as_bytes())
            .map_err(|_| "Could not write the local credential .env file.")?;
        file.sync_all()
            .map_err(|_| "Could not write the local credential .env file.")?;
        fs::rename(&temporary, path)
            .map_err(|_| "Could not write the local credential .env file.")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn manage_legacy_at(
    path: &Path,
    request: CredentialRequest,
) -> Result<Vec<SavedApiKey>, &'static str> {
    request.validate()?;
    let _lock = crate::local_lock::acquire(&path.with_extension("lock"), || false)
        .map_err(|_| "Local credentials are busy or unavailable. Try again.")?;
    let mut stored = read_legacy(path)?;
    match request {
        CredentialRequest::List {} => {}
        CredentialRequest::Save { name, value, id } => {
            if let Some(id) = id {
                if !stored.values.contains_key(&id) {
                    return Err(
                        "This API key no longer exists. Reload settings before replacing it.",
                    );
                }
                stored.values.remove(&id);
                stored.names.remove(&id);
            }
            let id = uuid::Uuid::new_v4().to_string();
            stored.names.insert(id.clone(), name.trim().into());
            stored.values.insert(id, value);
            write_legacy(path, &stored)?;
        }
        CredentialRequest::Remove { id } => {
            stored.values.remove(&id);
            stored.names.remove(&id);
            write_legacy(path, &stored)?;
        }
    }
    Ok(list_legacy(&stored))
}

#[cfg(test)]
fn read_metadata(path: &Path) -> Result<CredentialMetadata, &'static str> {
    match fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|_| "Could not read saved credential metadata.")
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Default::default()),
        Err(_) => Err("Could not read saved credential metadata."),
    }
}

#[cfg(test)]
fn write_metadata(path: &Path, metadata: &CredentialMetadata) -> Result<(), &'static str> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| "Could not write saved credential metadata.")?;
    use std::io::Write;
    let result = (|| -> Result<(), &'static str> {
        file.write_all(
            &serde_json::to_vec(metadata)
                .map_err(|_| "Could not write saved credential metadata.")?,
        )
        .map_err(|_| "Could not write saved credential metadata.")?;
        file.sync_all()
            .map_err(|_| "Could not write saved credential metadata.")?;
        fs::rename(&temporary, path).map_err(|_| "Could not write saved credential metadata.")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
fn list(metadata: &CredentialMetadata) -> Vec<SavedApiKey> {
    let mut keys: Vec<_> = metadata
        .names
        .iter()
        .map(|(id, name)| SavedApiKey {
            id: id.clone(),
            name: name.clone(),
        })
        .collect();
    keys.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    keys
}

#[cfg(test)]
fn manage_with(
    path: &Path,
    store: &impl SecretStore,
    request: CredentialRequest,
) -> Result<Vec<SavedApiKey>, &'static str> {
    request.validate()?;
    let mut metadata = read_metadata(path)?;
    match request {
        CredentialRequest::List {} => {}
        CredentialRequest::Save { name, value, id } => {
            if id
                .as_ref()
                .is_some_and(|existing| !metadata.names.contains_key(existing))
            {
                return Err("This API key no longer exists. Reload settings before replacing it.");
            }
            let new_id = uuid::Uuid::new_v4().to_string();
            store.put(&new_id, &value)?;
            let previous = id.inspect(|existing| {
                metadata.names.remove(existing);
            });
            metadata.names.insert(new_id.clone(), name.trim().into());
            if let Err(error) = write_metadata(path, &metadata) {
                let _ = store.remove(&new_id);
                return Err(error);
            }
            if let Some(previous) = previous {
                store.remove(&previous)?;
            }
        }
        CredentialRequest::Remove { id } => {
            metadata.names.remove(&id);
            write_metadata(path, &metadata)?;
            store.remove(&id)?;
        }
    }
    Ok(list(&metadata))
}

pub fn manage_api_keys(request: CredentialRequest) -> Result<Vec<SavedApiKey>, &'static str> {
    manage_legacy_at(&credentials_path()?, request)
}

#[cfg(target_os = "macos")]
fn native_put(service: &str, id: &str, value: &str) -> Result<(), &'static str> {
    let mut command = std::process::Command::new("/usr/bin/security");
    command.arg("-i");
    let input = format!(
        "add-generic-password -U -s {service} -a {id} -w {}\n",
        hex::encode(value.as_bytes())
    );
    if !native_process::run(command, Some(input.as_bytes()))?
        .status
        .success()
    {
        return Err("Could not save the secret in macOS Keychain.");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn native_get(service: &str, id: &str) -> Result<Option<String>, &'static str> {
    native_get_in_keychain(service, id, None)
}

#[cfg(target_os = "macos")]
fn native_get_in_keychain(
    service: &str,
    id: &str,
    keychain: Option<&Path>,
) -> Result<Option<String>, &'static str> {
    use std::process::Command;
    let mut command = Command::new("/usr/bin/security");
    command.args(["find-generic-password", "-s", service, "-a", id, "-w"]);
    if let Some(keychain) = keychain {
        command.arg(keychain);
    }
    let output = native_process::run(command, None)?;
    if output.status.code() == Some(44) {
        return Ok(None);
    }
    if !output.status.success() {
        return Err("Could not read macOS Keychain.");
    }
    let encoded = std::str::from_utf8(&output.stdout)
        .map_err(|_| "The saved secret has an unknown format.")?
        .trim();
    let bytes = hex::decode(encoded).map_err(|_| "The saved secret has an unknown format.")?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "The saved secret has an unknown format.")
}

#[cfg(target_os = "macos")]
fn native_contains(service: &str, id: &str) -> Result<bool, &'static str> {
    native_contains_in_keychain(service, id, None)
}

#[cfg(target_os = "macos")]
fn native_contains_in_keychain(
    service: &str,
    id: &str,
    keychain: Option<&Path>,
) -> Result<bool, &'static str> {
    use std::process::Command;
    let mut command = Command::new("/usr/bin/security");
    command.args(["find-generic-password", "-s", service, "-a", id]);
    if let Some(keychain) = keychain {
        command.arg(keychain);
    }
    let status = native_process::run(command, None)?.status;
    match status.code() {
        _ if status.success() => Ok(true),
        Some(44) => Ok(false),
        _ => Err("Could not inspect macOS Keychain."),
    }
}

#[cfg(target_os = "macos")]
fn native_remove(service: &str, id: &str) -> Result<(), &'static str> {
    use std::process::Command;
    let mut command = Command::new("/usr/bin/security");
    command.args(["delete-generic-password", "-s", service, "-a", id]);
    let status = native_process::run(command, None)?.status;
    if status.success() || status.code() == Some(44) {
        Ok(())
    } else {
        Err("Could not remove the secret from macOS Keychain.")
    }
}

#[cfg(not(target_os = "macos"))]
fn native_put(_service: &str, _id: &str, _value: &str) -> Result<(), &'static str> {
    Err("Saved secrets require the native macOS Keychain.")
}

#[cfg(not(target_os = "macos"))]
fn native_get(_service: &str, _id: &str) -> Result<Option<String>, &'static str> {
    Err("Saved secrets require the native macOS Keychain.")
}

#[cfg(not(target_os = "macos"))]
fn native_contains(_service: &str, _id: &str) -> Result<bool, &'static str> {
    Err("Saved secrets require the native macOS Keychain.")
}

#[cfg(not(target_os = "macos"))]
fn native_remove(_service: &str, _id: &str) -> Result<(), &'static str> {
    Err("Saved secrets require the native macOS Keychain.")
}

#[cfg(test)]
#[path = "credentials/tests.rs"]
mod tests;

mod access;
pub(crate) use access::LegacyAccess;

#[cfg(target_os = "macos")]
mod native_process;
