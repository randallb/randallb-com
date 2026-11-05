use super::EntityBinding as EntityCatalog;
use crate::ref_read::{RefReadError, RefRevision};
use crate::{ref_read::RefRecord, CwpdbResult as CwrResult};
use serde_json::Value;

#[derive(Debug, PartialEq)]
pub enum InsertOutcome {
    Created(RefRecord),
    Existing(RefRecord),
}
#[derive(Debug, PartialEq, Eq)]
pub enum PurgeOutcome {
    Purged,
    Absent,
}

impl EntityCatalog {
    /// Internal disposable insertion, with explicit equality reconciliation.
    /// Validates even an equal retry; a different value is never an upsert.
    pub(crate) fn insert_disposable_in_transaction(
        &self,
        tx: &rusqlite::Transaction<'_>,
        definition: &str,
        reference: &str,
        value: &Value,
    ) -> CwrResult<InsertOutcome> {
        self.require_disposable(definition)?;
        self.validate_reference(reference)?;
        self.validate(definition, value)?;
        match crate::ref_read::get_ref(tx, reference) {
            Ok(_) => {
                let current = self.get_in_connection(tx, definition, reference)?;
                if &current.value.properties != value {
                    return Err("entity value conflict".into());
                }
                Ok(InsertOutcome::Existing(current))
            }
            Err(RefReadError::NotFound) => Ok(InsertOutcome::Created(
                self.create_in_transaction(tx, definition, reference, value)?,
            )),
            Err(e) => Err(e.into()),
        }
    }

    /// Purge reclaims disposable identities; it never retires durable values.
    /// Missing values return Absent, but an old token cannot purge a new value.
    pub(crate) fn purge_in_transaction(
        &self,
        tx: &rusqlite::Transaction<'_>,
        definition: &str,
        reference: &str,
        expected: &RefRevision,
    ) -> CwrResult<PurgeOutcome> {
        self.require_disposable(definition)?;
        self.validate_reference(reference)?;
        match crate::ref_read::get_ref(tx, reference) {
            Err(RefReadError::NotFound) => return Ok(PurgeOutcome::Absent),
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }
        self.get_in_connection(tx, definition, reference)?;
        crate::disposable::purge_disposable_entities(tx, &[(reference, expected)])?;
        Ok(PurgeOutcome::Purged)
    }

    /// One atomic selection for acknowledgement/retention. The domain owns the
    /// surrounding transaction, including counters; no HTTP runs in it.
    pub(crate) fn purge_batch_in_transaction(
        &self,
        tx: &rusqlite::Transaction<'_>,
        definition: &str,
        records: &[(&str, &RefRevision)],
    ) -> CwrResult<()> {
        self.require_disposable(definition)?;
        if records.len() > 250 {
            return Err("disposable batch limit exceeded".into());
        }
        let mut bytes = 0usize;
        for (reference, _) in records {
            self.validate_reference(reference)?;
            let record = self.get_in_connection(tx, definition, reference)?;
            bytes += serde_json::to_vec(&record.value.properties)?.len();
            if bytes > 1_048_576 {
                return Err("disposable batch byte limit exceeded".into());
            }
        }
        crate::disposable::purge_disposable_entities(tx, records)
    }
}

#[cfg(test)]
#[path = "disposable_tests.rs"]
mod tests;
