//! Complete declared entity operations; no transaction crosses the storage boundary.
use super::{EntityBinding, PurgeOutcome};
use crate::{
    ref_read::{RefRecord, RefRevision},
    CwpdbResult, NativeStore,
};
use serde_json::Value;
impl NativeStore {
    pub fn create_declared_entity(
        &mut self,
        binding: &EntityBinding,
        reference: &str,
        value: &Value,
    ) -> CwpdbResult<RefRecord> {
        self.graph_transaction(|tx| {
            binding.create_in_transaction(tx, &binding.contract.definition_ref, reference, value)
        })
    }
    pub fn read_declared_entity(
        &self,
        binding: &EntityBinding,
        reference: &str,
    ) -> CwpdbResult<RefRecord> {
        binding.validate_reference(reference)?;
        let record = self.get_entity(reference)?;
        if record.kind != binding.contract.kind
            || record.value.schema_ref != binding.contract.schema_ref
        {
            return Err("entity definition mismatch".into());
        }
        binding.validate(&binding.contract.definition_ref, &record.value.properties)?;
        Ok(record)
    }
    pub fn replace_declared_entity(
        &mut self,
        binding: &EntityBinding,
        reference: &str,
        expected: &RefRevision,
        value: &Value,
    ) -> CwpdbResult<RefRecord> {
        self.graph_transaction(|tx| {
            binding.replace_in_transaction(
                tx,
                &binding.contract.definition_ref,
                reference,
                expected,
                value,
            )
        })
    }
    pub fn remove_declared_entity(
        &mut self,
        binding: &EntityBinding,
        reference: &str,
        expected: &RefRevision,
    ) -> CwpdbResult<()> {
        self.graph_transaction(|tx| {
            binding.remove_in_transaction(tx, &binding.contract.definition_ref, reference, expected)
        })
    }
    pub fn purge_declared_entity(
        &mut self,
        binding: &EntityBinding,
        reference: &str,
        expected: &RefRevision,
    ) -> CwpdbResult<PurgeOutcome> {
        self.graph_transaction(|tx| {
            binding.purge_in_transaction(tx, &binding.contract.definition_ref, reference, expected)
        })
    }
}

impl EntityBinding {
    pub(crate) fn merge_in_transaction(
        &self,
        tx: &rusqlite::Transaction<'_>,
        definition: &str,
        reference: &str,
        patch: &Value,
    ) -> CwpdbResult<()> {
        self.validate_reference(reference)?;
        if self.contract.disposable {
            return Err("durable domain merge requires a durable definition".into());
        }
        let mut previous =
            crate::node_props(tx, reference)?.unwrap_or_else(|| serde_json::json!({}));
        if patch.get("record").is_some() {
            if let Some(object) = previous.as_object_mut() {
                object.remove("record");
            }
        }
        let merged: String = tx.query_row(
            "SELECT json(json_patch(?1,?2))",
            rusqlite::params![
                serde_json::to_string(&previous)?,
                serde_json::to_string(patch)?
            ],
            |row| row.get(0),
        )?;
        self.validate(definition, &serde_json::from_str(&merged)?)?;
        crate::put_node_with_schema(
            tx,
            reference,
            &self.contract.kind,
            &self.contract.schema_ref,
            patch,
        )?;
        Ok(())
    }
}
impl NativeStore {
    pub fn merge_declared_entity(
        &mut self,
        binding: &EntityBinding,
        reference: &str,
        patch: &Value,
    ) -> CwpdbResult<()> {
        self.graph_transaction(|tx| {
            binding.merge_in_transaction(tx, &binding.contract.definition_ref, reference, patch)
        })
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "merge_tests.rs"]
mod merge_tests;
