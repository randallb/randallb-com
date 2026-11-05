//! Product metadata projection; persistence stays behind cwpdb's named port.
use super::{
    product_state_directory, EnvironmentEntry, EnvironmentEntryKind, EnvironmentEntryStatus,
    WorkspaceEnvironment, WorkspaceIdentity,
};
use crate::{credentials::SecretStore, cwpdb::environment::EnvironmentStore, CwrResult};
use std::fs::File;
pub(super) fn acquire_operation_lock() -> CwrResult<File> {
    crate::local_lock::acquire(
        &product_state_directory()?.join("workspace-environment.lock"),
        || false,
    )
}
pub(super) fn report(
    db: &EnvironmentStore,
    workspace: &WorkspaceIdentity,
    store: &impl SecretStore,
) -> CwrResult<WorkspaceEnvironment> {
    let snapshot = db.snapshot(&workspace.reference)?;
    let mut entries = Vec::new();
    for entry in snapshot.entries {
        let record = super::entry_read::decode_record(entry.value)?;
        let status = match (record.kind, record.credential_id.as_deref()) {
            (EnvironmentEntryKind::Variable, _) => EnvironmentEntryStatus::Available,
            (EnvironmentEntryKind::Secret, None) => EnvironmentEntryStatus::Missing,
            (EnvironmentEntryKind::Secret, Some(id)) => match store.contains(id) {
                Ok(true) => EnvironmentEntryStatus::Stored,
                Ok(false) => EnvironmentEntryStatus::Missing,
                Err(_) => EnvironmentEntryStatus::Unavailable,
            },
        };
        entries.push(EnvironmentEntry {
            reference: entry.reference,
            name: record.name,
            kind: record.kind,
            revision: entry.revision,
            value: record.value,
            configured: matches!(
                status,
                EnvironmentEntryStatus::Available | EnvironmentEntryStatus::Stored
            ),
            status,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.reference.cmp(&b.reference)));
    Ok(WorkspaceEnvironment {
        storage_identity: workspace.storage_identity.clone(),
        entries,
        cleanup_pending: snapshot.cleanup_pending,
        refresh_unavailable: false,
    })
}
