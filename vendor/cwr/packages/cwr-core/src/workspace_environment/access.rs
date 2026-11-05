//! Selected-entry reads without cleanup, Keychain probes, or recursive locking.
use super::*;
use std::fs::File;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct EntryMetadata {
    pub revision: i64,
    pub kind: EnvironmentEntryKind,
    pub credential_id: Option<String>,
}

pub(crate) struct EnvironmentAccess {
    _lock: File,
    db: EnvironmentStore,
    workspace: WorkspaceIdentity,
}

impl EnvironmentAccess {
    pub(crate) fn open(path: &Path, cancelled: impl FnMut() -> bool) -> CwrResult<Self> {
        Self::open_canonical(&path.canonicalize()?, cancelled)
    }
    /// The connection target already canonicalized this path before taking its
    /// grant lock. Never re-enter a potentially offline workspace mount here.
    pub(crate) fn open_canonical(path: &Path, cancelled: impl FnMut() -> bool) -> CwrResult<Self> {
        let lock = crate::local_lock::acquire(
            &product_state_directory()?.join("workspace-environment.lock"),
            cancelled,
        )?;
        let (mut db, identity) = open_database()?;
        let workspace = open_workspace(&mut db, path, identity)?;
        Ok(Self {
            _lock: lock,
            db,
            workspace,
        })
    }
    pub(crate) fn entries(&self) -> CwrResult<Vec<EnvironmentEntry>> {
        Ok(report(&self.db, &self.workspace, &MetadataOnly)?.entries)
    }
    pub(crate) fn metadata(&self, reference: &str) -> CwrResult<EntryMetadata> {
        let entry = read_entry(&self.db, &self.workspace.reference, reference)?
            .ok_or("The assigned workspace entry is unavailable.")?;
        Ok(EntryMetadata {
            revision: entry.revision,
            kind: entry.kind,
            credential_id: entry.credential_id,
        })
    }
    pub(crate) fn variable(&self, reference: &str) -> CwrResult<String> {
        let entry = read_entry(&self.db, &self.workspace.reference, reference)?
            .ok_or("The assigned workspace entry is unavailable.")?;
        if entry.kind != EnvironmentEntryKind::Variable {
            return Err("Expected a workspace variable.".into());
        }
        entry
            .value
            .ok_or_else(|| "The assigned workspace variable is unavailable.".into())
    }
}

/// Includes the namespace path, so a copied database is not execution authority.
pub(crate) fn identity() -> CwrResult<String> {
    let namespace = product_state_directory_path()?;
    let label = env::var("WORKLOOP_STORAGE_IDENTITY").unwrap_or_default();
    Ok(hex::encode(Sha256::digest(format!(
        "{}\0{label}",
        namespace.display()
    ))))
}

struct MetadataOnly;
impl SecretStore for MetadataOnly {
    fn contains(&self, _id: &str) -> Result<bool, &'static str> {
        Ok(true)
    }
    fn get(&self, _id: &str) -> Result<Option<String>, &'static str> {
        Err("Metadata view cannot read secrets.")
    }
    fn put(&self, _id: &str, _value: &str) -> Result<(), &'static str> {
        Err("Metadata view cannot write secrets.")
    }
    fn remove(&self, _id: &str) -> Result<(), &'static str> {
        Err("Metadata view cannot delete secrets.")
    }
}
