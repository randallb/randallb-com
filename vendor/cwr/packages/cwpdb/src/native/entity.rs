//! Minimal declared graph mutations; not the full request-replay createRef API.
use super::NativeStore;
use crate::{
    identity_claim, put_node_with_schema,
    ref_read::{get_ref, RefReadError, RefRecord},
    CwpdbResult,
};
use rusqlite::TransactionBehavior;
use serde_json::Value;

impl NativeStore {
    /// Atomically insert an already-authorized, schema-validated entity. The
    /// caller owns the store and domain scope. No edges or children are created.
    /// A lost response is reconciled by reading the caller-selected ref; this
    /// primitive does not implement retained request-ID replay.
    pub fn create_entity(
        &mut self,
        reference: &str,
        schema: &str,
        value: &Value,
    ) -> CwpdbResult<RefRecord> {
        self.create_entity_with_kind(reference, "declared_entity", schema, value)
    }

    /// Insert using a trusted declaration's storage record kind.
    pub fn create_entity_with_kind(
        &mut self,
        reference: &str,
        kind: &str,
        schema: &str,
        value: &Value,
    ) -> CwpdbResult<RefRecord> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = insert_entity(&tx, reference, kind, schema, value)?;
        tx.commit()?;
        Ok(result)
    }

    /// Read authoritative graph state without starting a write transaction.
    pub fn get_entity(&self, reference: &str) -> CwpdbResult<RefRecord> {
        Ok(get_ref(&self.connection, reference)?)
    }
}

/// Insert a schema-validated record within the owning domain's transaction.
/// The caller supplies the declared storage binding and commits the transaction.
pub fn insert_entity(
    tx: &rusqlite::Transaction<'_>,
    reference: &str,
    kind: &str,
    schema: &str,
    value: &Value,
) -> CwpdbResult<RefRecord> {
    if !value.is_object() {
        return Err("entity value must be an object".into());
    }
    match get_ref(tx, reference) {
        Ok(_) => return Err("already_exists".into()),
        Err(RefReadError::WrongRecordType) => return Err("wrong_record_type".into()),
        Err(RefReadError::NotFound) => {}
        Err(error) => return Err(error.into()),
    }
    if identity_claim(tx, reference)?.is_some() {
        return Err("ref_retired_or_claimed".into());
    }
    put_node_with_schema(tx, reference, kind, schema, value)?;
    let result = get_ref(tx, reference)?;
    Ok(result)
}

/// Replace the complete object in a caller-owned transaction, without merging
/// omitted keys or treating explicit nulls as deletions. No upsert is allowed.
pub fn replace_entity(
    tx: &rusqlite::Transaction<'_>,
    reference: &str,
    kind: &str,
    schema: &str,
    expected: &crate::ref_read::RefRevision,
    value: &Value,
) -> CwpdbResult<RefRecord> {
    if !value.is_object() {
        return Err("entity value must be an object".into());
    }
    let current = get_ref(tx, reference)?;
    if current.kind != kind || current.value.schema_ref != schema {
        return Err("entity definition mismatch".into());
    }
    if &current.revision != expected {
        return Err("entity revision conflict".into());
    }
    let changed = tx.execute(
        "UPDATE nodes SET props=jsonb(?1),revision=revision+1
         WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)
           AND revision=?3 AND kind=?4
           AND schema_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?5)",
        rusqlite::params![
            serde_json::to_string(value)?,
            reference,
            expected.legacy_counter(),
            kind,
            schema
        ],
    )?;
    if changed != 1 {
        return Err("entity revision conflict".into());
    }
    Ok(get_ref(tx, reference)?)
}
