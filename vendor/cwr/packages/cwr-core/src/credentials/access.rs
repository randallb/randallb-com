use super::*;
use std::fs::File;

/// Values remain private to the caller and are never serialized or debugged.
pub(crate) struct LegacyAccess {
    _lock: File,
    keys: StoredKeys,
}
impl LegacyAccess {
    pub(crate) fn open(cancelled: impl FnMut() -> bool) -> crate::CwrResult<Self> {
        let path = credentials_path()?;
        let lock = crate::local_lock::acquire(&path.with_extension("lock"), cancelled)?;
        Ok(Self {
            _lock: lock,
            keys: read_legacy(&path)?,
        })
    }
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.keys.values.contains_key(id) && self.keys.names.contains_key(id)
    }
    pub(crate) fn value(&self, id: &str) -> crate::CwrResult<String> {
        self.keys
            .values
            .get(id)
            .cloned()
            .ok_or_else(|| "The saved API key is unavailable.".into())
    }
}
