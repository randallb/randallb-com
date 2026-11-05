//! Trusted declarations and pure complete-value validation; no native handle crosses this seam.
use crate::{
    ref_read::{RefRecord, RefRevision},
    CwpdbResult,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
#[derive(Clone, Debug)]
pub struct EntityContract {
    pub definition_ref: String,
    pub kind: String,
    pub schema_ref: String,
    pub disposable: bool,
}
pub trait ValueValidator: Send + Sync {
    fn validate_reference(&self, reference: &str) -> CwpdbResult<()>;
    fn validate(&self, definition_ref: &str, complete_properties: &Value) -> CwpdbResult<()>;
}
#[derive(Clone)]
pub struct Catalog {
    contracts: BTreeMap<String, EntityContract>,
    reserved: Arc<BTreeSet<String>>,
    validator: Arc<dyn ValueValidator>,
}
impl Catalog {
    pub fn new(
        contracts: Vec<EntityContract>,
        validator: Arc<dyn ValueValidator>,
    ) -> CwpdbResult<Self> {
        let mut values = BTreeMap::new();
        for contract in contracts {
            if values
                .insert(contract.definition_ref.clone(), contract)
                .is_some()
            {
                return Err("duplicate entity contract".into());
            }
        }
        Ok(Self {
            reserved: Arc::new(values.keys().cloned().collect()),
            contracts: values,
            validator,
        })
    }
    pub(crate) fn validate_reference(&self, reference: &str) -> CwpdbResult<()> {
        self.validator.validate_reference(reference)
    }
    pub fn entity(&self, definition: &str) -> CwpdbResult<EntityBinding> {
        let contract = self
            .contracts
            .get(definition)
            .ok_or("unknown entity definition")?
            .clone();
        Ok(EntityBinding {
            contract,
            validator: self.validator.clone(),
            reserved: self.reserved.clone(),
        })
    }
}
pub struct EntityBinding {
    reserved: Arc<BTreeSet<String>>,
    contract: EntityContract,
    validator: Arc<dyn ValueValidator>,
}
impl EntityBinding {
    pub(crate) fn validate(&self, definition: &str, value: &Value) -> CwpdbResult<()> {
        if definition != self.contract.definition_ref {
            return Err("entity definition mismatch".into());
        }
        self.validator.validate(definition, value)
    }
    pub(crate) fn get_in_connection(
        &self,
        db: &rusqlite::Connection,
        definition: &str,
        reference: &str,
    ) -> CwpdbResult<RefRecord> {
        self.validator.validate_reference(reference)?;
        let record = crate::ref_read::get_ref(db, reference)?;
        if record.kind != self.contract.kind || record.value.schema_ref != self.contract.schema_ref
        {
            return Err("entity definition mismatch".into());
        }
        self.validate(definition, &record.value.properties)?;
        Ok(record)
    }
    pub(crate) fn create_in_transaction(
        &self,
        tx: &rusqlite::Transaction<'_>,
        definition: &str,
        reference: &str,
        value: &Value,
    ) -> CwpdbResult<RefRecord> {
        self.validator.validate_reference(reference)?;
        self.validate(definition, value)?;
        if self.reserved.contains(reference) {
            return Err("entity ref is reserved for a definition".into());
        }
        if self.contract.disposable {
            return crate::disposable::insert_disposable_entity(
                tx,
                reference,
                &self.contract.kind,
                &self.contract.schema_ref,
                value,
            );
        }
        crate::insert_entity(
            tx,
            reference,
            &self.contract.kind,
            &self.contract.schema_ref,
            value,
        )
    }
    pub(crate) fn replace_in_transaction(
        &self,
        tx: &rusqlite::Transaction<'_>,
        definition: &str,
        reference: &str,
        expected: &RefRevision,
        value: &Value,
    ) -> CwpdbResult<RefRecord> {
        self.get_in_connection(tx, definition, reference)?;
        self.validate(definition, value)?;
        if self.contract.disposable {
            return crate::disposable::replace_disposable_entity(
                tx,
                reference,
                &self.contract.kind,
                &self.contract.schema_ref,
                expected,
                value,
            );
        }
        crate::replace_entity(
            tx,
            reference,
            &self.contract.kind,
            &self.contract.schema_ref,
            expected,
            value,
        )
    }
}

impl EntityBinding {
    pub(crate) fn validate_reference(&self, reference: &str) -> CwpdbResult<()> {
        self.validator.validate_reference(reference)
    }
    fn require_disposable(&self, definition: &str) -> CwpdbResult<()> {
        if definition != self.contract.definition_ref || !self.contract.disposable {
            return Err("entity definition is not disposable".into());
        }
        Ok(())
    }
}
mod disposable;
pub use disposable::{InsertOutcome, PurgeOutcome};

impl EntityBinding {
    pub(crate) fn remove_in_transaction(
        &self,
        tx: &rusqlite::Transaction<'_>,
        definition: &str,
        reference: &str,
        expected: &RefRevision,
    ) -> CwpdbResult<()> {
        if self.contract.disposable {
            return Err("disposable entities require purge".into());
        }
        let current = self.get_in_connection(tx, definition, reference)?;
        if &current.revision != expected {
            return Err("entity revision conflict".into());
        }
        crate::remove(tx, reference)
    }
}

mod native;
