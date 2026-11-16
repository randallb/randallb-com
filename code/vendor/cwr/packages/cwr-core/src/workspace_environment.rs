//! Workloop product storage for workspace variables and secret references.

use crate::{credentials::SecretStore, cwpdb, CwrResult};
use cwpdb::environment::EnvironmentStore;
#[cfg(test)]
use recovery::cleanup_reference;
use recovery::{add_cleanup, recover, remove_cleanup};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};
use storage::{acquire_operation_lock, report};
use validation::{validate_name, validate_record, validate_value, EntryRecord};

mod declared_secret;
mod declared_variable;
mod entry_read;
use entry_read::read_entry;
mod recovery;
mod storage;
mod validation;

#[cfg(test)]
pub(crate) static TEST_ENVIRONMENT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentEntryKind {
    Variable,
    Secret,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentEntryStatus {
    Available,
    Stored,
    Missing,
    Unavailable,
}

impl EnvironmentEntryKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Variable => "variable",
            Self::Secret => "secret",
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct EnvironmentEntry {
    pub reference: String,
    pub name: String,
    pub kind: EnvironmentEntryKind,
    pub revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub configured: bool,
    pub status: EnvironmentEntryStatus,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceEnvironment {
    pub storage_identity: String,
    pub entries: Vec<EnvironmentEntry>,
    pub cleanup_pending: bool,
    pub refresh_unavailable: bool,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnvironmentRequest {
    List {
        path: PathBuf,
    },
    Save {
        path: PathBuf,
        name: String,
        kind: EnvironmentEntryKind,
        value: String,
        reference: Option<String>,
        expected_revision: Option<i64>,
    },
    Remove {
        path: PathBuf,
        reference: String,
        expected_revision: i64,
    },
}

pub fn manage(request: EnvironmentRequest) -> CwrResult<WorkspaceEnvironment> {
    let store = workspace_secret_store()?;
    match request {
        EnvironmentRequest::List { path } => list(&path, &store),
        EnvironmentRequest::Save {
            path,
            name,
            kind,
            value,
            reference,
            expected_revision,
        } => save(
            &path,
            &store,
            &name,
            kind,
            &value,
            reference.as_deref(),
            expected_revision,
        ),
        EnvironmentRequest::Remove {
            path,
            reference,
            expected_revision,
        } => remove(&path, &store, &reference, expected_revision),
    }
}

pub(crate) fn list(path: &Path, store: &impl SecretStore) -> CwrResult<WorkspaceEnvironment> {
    let _lock = acquire_operation_lock()?;
    let (db, workspace) = open_after_recovery(path, store)?;
    report(&db, &workspace, store)
}

pub(crate) fn save(
    path: &Path,
    store: &impl SecretStore,
    name: &str,
    kind: EnvironmentEntryKind,
    value: &str,
    reference: Option<&str>,
    expected_revision: Option<i64>,
) -> CwrResult<WorkspaceEnvironment> {
    validate_name(name)?;
    validate_value(value, kind)?;
    let _lock = acquire_operation_lock()?;
    let (mut db, workspace) = open_after_recovery(path, store)?;
    let existing = reference
        .map(|reference| read_entry(&db, &workspace.reference, reference))
        .transpose()?
        .flatten();
    match (reference, &existing, expected_revision) {
        (None, None, None) => {}
        (Some(_), Some(entry), Some(expected))
            if entry.revision == expected && entry.kind == kind => {}
        _ => return stale(),
    }
    ensure_unique_name(&db, &workspace.reference, name, reference)?;
    let reference = reference.map(str::to_owned).unwrap_or_else(|| {
        format!(
            "cwp://org.coworkerprotocol.local/wlp/environment-entries/{}",
            uuid::Uuid::new_v4()
        )
    });
    let old_credential = existing
        .as_ref()
        .and_then(|entry| entry.credential_id.clone());
    let new_credential = if kind == EnvironmentEntryKind::Secret {
        let id = uuid::Uuid::new_v4().to_string();
        add_cleanup(&mut db, &workspace.reference, &id)?;
        if let Err(error) = store.put(&id, value) {
            if store.remove(&id).is_ok() {
                remove_cleanup(&mut db, &workspace.reference, &id)?;
            }
            return Err(error.into());
        }
        Some(id)
    } else {
        None
    };
    let next = cwpdb::environment::EntryValue {
        name: name.into(),
        kind: kind.as_str().into(),
        value: if kind == EnvironmentEntryKind::Variable {
            Some(value.into())
        } else {
            None
        },
        credential_id: new_credential.clone(),
    };
    let commit = (|| -> CwrResult<()> {
        let envelope = serde_json::json!({"record":&next});
        if kind == EnvironmentEntryKind::Variable {
            declared_variable::validate(&envelope)?;
        } else {
            declared_secret::validate(&envelope)?;
        }
        db.save(cwpdb::environment::Save {
            workspace: &workspace.reference,
            reference: &reference,
            expected_revision,
            value: &next,
        })
    })();
    if let Err(error) = commit {
        if let Some(id) = &new_credential {
            let _ = store.remove(id);
        }
        return Err(error);
    }
    if let Some(old) = old_credential.filter(|old| Some(old) != new_credential.as_ref()) {
        if store.remove(&old).is_ok() {
            let _ = remove_cleanup(&mut db, &workspace.reference, &old);
        }
    }
    Ok(committed_report(&db, &workspace, store))
}

pub(crate) fn remove(
    path: &Path,
    store: &impl SecretStore,
    reference: &str,
    expected_revision: i64,
) -> CwrResult<WorkspaceEnvironment> {
    let _lock = acquire_operation_lock()?;
    let (mut db, workspace) = open_after_recovery(path, store)?;
    let entry = read_entry(&db, &workspace.reference, reference)?
        .ok_or("This environment entry no longer exists.")?;
    if entry.revision != expected_revision {
        return stale();
    }
    db.remove(&workspace.reference, reference, expected_revision)?;
    if let Some(id) = entry.credential_id {
        if store.remove(&id).is_ok() {
            let _ = remove_cleanup(&mut db, &workspace.reference, &id);
        }
    }
    Ok(committed_report(&db, &workspace, store))
}

#[cfg(test)]
pub(crate) fn resolve_with(
    path: &Path,
    store: &impl SecretStore,
    reference: &str,
    expected_revision: i64,
) -> CwrResult<String> {
    let _lock = acquire_operation_lock()?;
    let (db, workspace) = open_after_recovery(path, store)?;
    let entry = read_entry(&db, &workspace.reference, reference)?
        .ok_or("The assigned workspace entry is unavailable.")?;
    if entry.revision != expected_revision {
        return Err("Workspace settings changed. Start this work again.".into());
    }
    match entry.kind {
        EnvironmentEntryKind::Variable => entry
            .value
            .ok_or_else(|| "The workspace variable has an unknown format.".into()),
        EnvironmentEntryKind::Secret => store
            .get(
                entry
                    .credential_id
                    .as_deref()
                    .ok_or("The workspace secret has an unknown format.")?,
            )
            .map_err(|_| "The workspace secret is unavailable. Unlock Keychain or replace it.")?
            .ok_or_else(|| {
                "The workspace secret is unavailable. Unlock Keychain or replace it.".into()
            }),
    }
}

pub fn entries(path: &Path) -> CwrResult<Vec<EnvironmentEntry>> {
    if !database_exists()? {
        return Ok(vec![]);
    }
    EnvironmentAccess::open(path, || false)?.entries()
}

pub(crate) fn database_exists() -> CwrResult<bool> {
    Ok(EnvironmentStore::exists(&database_directory_path()?))
}

pub(super) struct WorkspaceIdentity {
    reference: String,
    storage_identity: String,
}

fn open_after_recovery(
    path: &Path,
    store: &impl SecretStore,
) -> CwrResult<(EnvironmentStore, WorkspaceIdentity)> {
    let (mut db, storage_identity) = open_database()?;
    recover(&mut db, store)?;
    let workspace = open_workspace(&mut db, &path.canonicalize()?, storage_identity)?;
    Ok((db, workspace))
}

#[cfg(test)]
fn open(path: &Path) -> CwrResult<(EnvironmentStore, WorkspaceIdentity)> {
    let (mut db, storage_identity) = open_database()?;
    let workspace = open_workspace(&mut db, &path.canonicalize()?, storage_identity)?;
    Ok((db, workspace))
}

fn open_database() -> CwrResult<(EnvironmentStore, String)> {
    let storage_identity = configured_storage_identity()?;
    let store = EnvironmentStore::open(&database_directory()?, &storage_identity)?;
    Ok((store, storage_identity))
}

fn open_workspace(
    db: &mut EnvironmentStore,
    path: &Path,
    storage_identity: String,
) -> CwrResult<WorkspaceIdentity> {
    let path_hash = hex::encode(Sha256::digest(path.as_os_str().as_encoded_bytes()));
    let workspace = WorkspaceIdentity {
        reference: format!("cwp://org.coworkerprotocol.local/wlp/workspaces/{path_hash}"),
        storage_identity,
    };
    db.bind_workspace(&workspace.reference, &path_hash)?;
    Ok(workspace)
}

pub(crate) fn product_state_directory() -> CwrResult<PathBuf> {
    let directory = product_state_directory_path()?;
    fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::symlink_metadata(&directory)?;
        // SAFETY: geteuid reads process credentials and has no preconditions.
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::other("WLP DB directory is not privately owned").into());
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(directory)
}

pub(crate) fn product_state_directory_path() -> CwrResult<PathBuf> {
    let state = env::var_os("WORKLOOP_STATE_HOME")
        .filter(|value| Path::new(value).is_absolute())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_STATE_HOME")
                .filter(|value| Path::new(value).is_absolute())
                .map(PathBuf::from)
        })
        .or_else(dirs::state_dir)
        .or_else(dirs::data_local_dir)
        .ok_or("Could not determine the Workloop product-state directory.")?;
    Ok(state.join("WLP"))
}

fn database_directory() -> CwrResult<PathBuf> {
    product_state_directory()
}

fn database_directory_path() -> CwrResult<PathBuf> {
    product_state_directory_path()
}

pub(crate) fn workspace_secret_store() -> CwrResult<crate::credentials::NativeSecretStore> {
    Ok(crate::credentials::NativeSecretStore::for_namespace(
        &product_state_directory()?.join("credentials"),
    ))
}

fn ensure_unique_name(
    db: &EnvironmentStore,
    workspace: &str,
    name: &str,
    except: Option<&str>,
) -> CwrResult<()> {
    db.ensure_unique_name(workspace, name, except)
}

fn stale<T>() -> CwrResult<T> {
    Err("Workspace settings changed. Reload before saving.".into())
}

#[cfg(test)]
#[path = "workspace_environment/tests.rs"]
mod tests;

pub(crate) fn validate_environment_name(name: &str) -> CwrResult<()> {
    validation::validate_name(name)?;
    if matches!(
        name,
        "PATH"
            | "HOME"
            | "SHELL"
            | "ENV"
            | "BASH_ENV"
            | "ZDOTDIR"
            | "SHELLOPTS"
            | "BASHOPTS"
            | "NODE_OPTIONS"
            | "PYTHONPATH"
            | "PYTHONHOME"
    ) || name.starts_with("LD_")
        || name.starts_with("DYLD_")
        || name.starts_with("CWR_")
    {
        return Err(
            "This environment name controls process startup and cannot be assigned.".into(),
        );
    }
    Ok(())
}

mod access;
pub(crate) use access::{identity as storage_identity, EntryMetadata, EnvironmentAccess};

fn configured_storage_identity() -> CwrResult<String> {
    env::var("WORKLOOP_STORAGE_IDENTITY").ok().filter(|v| !v.is_empty() && v.len() <= 96 && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '.')))
        .ok_or_else(|| "Workloop storage identity is missing or invalid. Open this workspace through its configured host.".into())
}

// A committed mutation must never be presented as a failed save because an
// optional list refresh or cleanup failed. Callers retain their previous display
// and ask for a reload when this flag is set.
fn committed_report(
    db: &EnvironmentStore,
    workspace: &WorkspaceIdentity,
    store: &impl SecretStore,
) -> WorkspaceEnvironment {
    report(db, workspace, store).unwrap_or_else(|_| WorkspaceEnvironment {
        storage_identity: workspace.storage_identity.clone(),
        entries: vec![],
        cleanup_pending: true,
        refresh_unavailable: true,
    })
}
