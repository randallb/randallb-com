use super::{CwrResult, EnvironmentStore, SecretStore};
pub(super) fn add_cleanup(db: &mut EnvironmentStore, workspace: &str, id: &str) -> CwrResult<()> {
    db.reserve_cleanup(workspace, id)
}
pub(super) fn remove_cleanup(
    db: &mut EnvironmentStore,
    _workspace: &str,
    id: &str,
) -> CwrResult<()> {
    db.finish_cleanup(id)
}
pub(super) fn recover(db: &mut EnvironmentStore, store: &impl SecretStore) -> CwrResult<()> {
    for pending in db.cleanup_candidates()? {
        if pending.in_use || store.remove(&pending.credential_id).is_ok() {
            db.finish_cleanup(&pending.credential_id)?;
        }
    }
    Ok(())
}
#[cfg(test)]
pub(super) fn cleanup_reference(id: &str) -> String {
    format!("cwp://org.coworkerprotocol.local/wlp/credential-cleanups/{id}")
}
