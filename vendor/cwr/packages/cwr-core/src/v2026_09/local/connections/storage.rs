use super::*;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Grants {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) storage_version: Option<u8>,
    #[serde(default)]
    pub(super) connections: BTreeMap<String, Grant>,
    #[serde(default)]
    pub(super) plaintext: BTreeMap<String, PlaintextGrant>,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Backend {
    #[default]
    Legacy,
    Workspace,
}
impl Backend {
    fn is_legacy(&self) -> bool {
        *self == Self::Legacy
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Grant {
    #[serde(default, skip_serializing_if = "Backend::is_legacy")]
    pub(super) backend: Backend,
    pub(super) key_id: String,
    pub(super) fingerprint: String,
    pub(super) token: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PlaintextGrant {
    pub(super) fingerprint: String,
    pub(super) value: Value,
}

pub(super) fn legacy_grants_path(path: &Path) -> CwrResult<PathBuf> {
    let home = platform_state_home()?;
    #[cfg(test)]
    let home = std::env::var_os("CWR_TEST_LEGACY_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or(home);
    let root = home.join(application_directory_name());
    let directory = root.join("connections");
    Ok(directory.join(format!(
        "{}.toml",
        hex::encode(Sha256::digest(path.as_os_str().as_encoded_bytes()))
    )))
}

#[cfg(test)]
pub(super) fn grants_path(path: &Path) -> CwrResult<PathBuf> {
    scoped_grants_path(path)
}

fn scoped_grants_path(path: &Path) -> CwrResult<PathBuf> {
    let directory =
        crate::workspace_environment::product_state_directory_path()?.join("connections");
    Ok(directory.join(format!(
        "{}.toml",
        hex::encode(Sha256::digest(path.as_os_str().as_encoded_bytes()))
    )))
}

fn read_grants_file(path: &Path) -> CwrResult<Option<Grants>> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map(Some)
            .map_err(|_| invalid_error("Could not read local project connections.")),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(_) => invalid("Could not read local project connections."),
    }
}

pub(super) fn read_grants(path: &Path) -> CwrResult<Grants> {
    let scoped = read_grants_file(&scoped_grants_path(path)?)?;
    if let Some(mut authoritative) = scoped
        .clone()
        .filter(|grants| grants.storage_version == Some(1))
    {
        authoritative.storage_version = None;
        return Ok(authoritative);
    }
    if scoped.is_some() {
        return invalid("Unsupported connection storage version.");
    }
    Ok(read_grants_file(&legacy_grants_path(path)?)?.unwrap_or_default())
}

// Hold scoped coordination even during the first migration. A dispatcher using
// legacy data cannot race a writer that observes the newly authoritative file.
pub(super) fn acquire_grants_lock(path: &Path) -> CwrResult<Vec<File>> {
    acquire_grants_lock_with(path, || false)
}

pub(super) fn acquire_grants_lock_with(
    path: &Path,
    mut cancelled: impl FnMut() -> bool,
) -> CwrResult<Vec<File>> {
    let scoped = scoped_grants_path(path)?;
    create_grants_directory(&scoped)?;
    let mut locks = vec![crate::local_lock::acquire(
        &scoped.with_extension("lock"),
        &mut cancelled,
    )?];
    if read_grants_file(&scoped)?.is_none_or(|g| g.storage_version != Some(1)) {
        let legacy = legacy_grants_path(path)?;
        create_grants_directory(&legacy)?;
        locks.push(crate::local_lock::acquire(
            &legacy.with_extension("lock"),
            &mut cancelled,
        )?);
    }
    Ok(locks)
}

pub(super) fn reject_stale_grants(grants: &Grants, expected_revision: &str) -> CwrResult<()> {
    if grants_revision(grants)? == expected_revision {
        return Ok(());
    }
    invalid("Project connections changed. Review the latest choices before saving.")
}

pub(super) fn grants_revision(grants: &Grants) -> CwrResult<String> {
    Ok(hex::encode(Sha256::digest(toml::to_string(grants)?)))
}

pub(super) fn write_grants(path: &Path, grants: &Grants) -> CwrResult<()> {
    let scoped_path = scoped_grants_path(path)?;
    create_grants_directory(&scoped_path)?;
    let mut stored = grants.clone();
    stored.storage_version = Some(1);
    write_grants_file(&scoped_path, &stored)
}

pub(super) fn create_grants_directory(path: &Path) -> CwrResult<()> {
    let directory = path
        .parent()
        .ok_or("Connection metadata has no containing directory.")?;
    if let Some(root) = directory.parent() {
        fs::create_dir_all(root)?;
        create_private_directory(root)?;
    }
    create_private_directory(directory)
}

fn write_grants_file(path: &Path, grants: &Grants) -> CwrResult<()> {
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    use std::io::Write;
    let result = (|| -> CwrResult<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(toml::to_string(grants)?.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
