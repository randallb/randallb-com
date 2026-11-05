//! Local entity declaration experiment. This is not an actor-authorized host API.
//! Callers must already own the selected graph and store. Existing host privacy
//! and credential operations are not replaced or bypassed by this module.
use crate::{
    cwpdb::{ref_read::RefRecord, NativeStore},
    v2026_09::graph::resolve_graph,
    CwrRef, CwrResult, CwrRoot,
};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

pub const DEFINITION_TYPE: &str = "cwp://org.coworkerprotocol/2026-10-22/schemas/entity-definition";

pub mod personal_collections;
pub mod posthog;
pub mod relationships;
pub mod sidebar_pin_move;
pub mod sidebar_pins;
pub use crate::cwpdb::declared::{InsertOutcome, PurgeOutcome};

#[derive(Clone)]
pub struct EntityCatalog {
    definitions: BTreeMap<String, Definition>,
}

#[derive(Clone)]
struct Definition {
    validator: jsonschema::Validator,
    kind: String,
    schema: String,
    disposable: bool,
}

impl EntityCatalog {
    /// Load exported/imported declarations through the normal LoopGraph loader.
    /// Only cwpdb-backed values with an object schema are supported initially.
    pub fn load(graph: impl AsRef<Path>) -> CwrResult<Self> {
        let graph = resolve_graph(graph)?;
        let mut catalog = Self {
            definitions: BTreeMap::new(),
        };
        for resource in graph
            .resources
            .into_iter()
            .filter(|r| r.type_ref == DEFINITION_TYPE)
        {
            let mut document = resource.document;
            document["ref"] = Value::String(resource.reference.to_string());
            catalog.add(&document)?;
        }
        Ok(catalog)
    }

    /// Load a trusted bundled declaration without depending on source-tree paths.
    pub fn from_toml(source: &str) -> CwrResult<Self> {
        let document = serde_json::to_value(toml::from_str::<toml::Value>(source)?)?;
        let mut catalog = Self {
            definitions: BTreeMap::new(),
        };
        catalog.add(&document)?;
        Ok(catalog)
    }

    /// Compile a trusted module's bundled declarations through the same validator
    /// used for resources loaded by normal graph composition.
    pub fn from_toml_documents<'a>(sources: impl IntoIterator<Item = &'a str>) -> CwrResult<Self> {
        let mut catalog = Self {
            definitions: BTreeMap::new(),
        };
        for source in sources {
            let document = serde_json::to_value(toml::from_str::<toml::Value>(source)?)?;
            catalog.add(&document)?;
        }
        Ok(catalog)
    }

    fn add(&mut self, document: &Value) -> CwrResult<()> {
        let definition_schema: Value = serde_json::from_str(include_str!(
            "../resources/base/entities/definition.schema.json"
        ))?;
        let options = || {
            let options = jsonschema::options();
            options
                .with_format("cwr-ref", |value: &str| CwrRef::parse(value).is_ok())
                .should_validate_formats(true)
        };
        if !options().build(&definition_schema)?.is_valid(document) {
            return Err("invalid entity definition".into());
        }
        let reference = document["ref"].as_str().ok_or("missing definition ref")?;
        require_live_ref(reference)?;
        let schema = &document["value_schema"];
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err("entity definition must require an object value".into());
        }
        let definition = Definition {
            disposable: document["lifetime"].as_str() == Some("disposable"),
            validator: options()
                .build(schema)
                .map_err(|_| "invalid entity value schema")?,
            kind: document["record_kind"]
                .as_str()
                .unwrap_or("declared_entity")
                .to_owned(),
            schema: document["record_schema"]
                .as_str()
                .unwrap_or(reference)
                .to_owned(),
        };
        require_live_ref(&definition.schema)?;
        if self
            .definitions
            .insert(reference.to_owned(), definition)
            .is_some()
        {
            return Err("duplicate entity definition ref".into());
        }
        Ok(())
    }

    fn storage_catalog(&self) -> CwrResult<crate::cwpdb::declared::Catalog> {
        let contracts = self
            .types()
            .into_iter()
            .map(|definition| self.contract(definition))
            .collect::<CwrResult<Vec<_>>>()?;
        crate::cwpdb::declared::Catalog::new(contracts, std::sync::Arc::new(self.clone()))
    }

    /// Trusted physical binding; schema loading and value validation stay in CWR.
    pub fn contract(&self, definition: &str) -> CwrResult<crate::cwpdb::declared::EntityContract> {
        let binding = self
            .definitions
            .get(definition)
            .ok_or("unknown entity definition")?;
        Ok(crate::cwpdb::declared::EntityContract {
            definition_ref: definition.to_owned(),
            kind: binding.kind.clone(),
            schema_ref: binding.schema.clone(),
            disposable: binding.disposable,
        })
    }

    pub fn types(&self) -> Vec<&str> {
        self.definitions.keys().map(String::as_str).collect()
    }

    /// Create a validated local value. Validation precedes any store writes.
    pub fn create(
        &self,
        store: &mut NativeStore,
        definition: &str,
        reference: &str,
        value: &Value,
    ) -> CwrResult<RefRecord> {
        store.create_declared_entity(
            &self.storage_catalog()?.entity(definition)?,
            reference,
            value,
        )
    }

    pub fn get(
        &self,
        store: &NativeStore,
        definition: &str,
        reference: &str,
    ) -> CwrResult<RefRecord> {
        store.read_declared_entity(&self.storage_catalog()?.entity(definition)?, reference)
    }

    pub(crate) fn validate(&self, definition: &str, value: &Value) -> CwrResult<()> {
        let validator = self
            .definitions
            .get(definition)
            .ok_or("unknown entity definition")?;
        if !validator.validator.is_valid(value) {
            return Err("entity value does not satisfy its definition".into());
        }
        Ok(())
    }
}

pub(crate) fn require_live_ref(reference: &str) -> CwrResult<()> {
    let parsed = CwrRef::parse(reference)?;
    if !matches!(parsed.root(), CwrRoot::Named(_))
        || parsed.checkpoint().is_some()
        || parsed.fragment().is_some()
    {
        return Err("entity refs must be absolute, named, live refs without fragments".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "entities_tests.rs"]
mod tests;

impl crate::cwpdb::declared::ValueValidator for EntityCatalog {
    fn validate_reference(&self, reference: &str) -> CwrResult<()> {
        require_live_ref(reference)
    }
    fn validate(&self, definition_ref: &str, complete_properties: &Value) -> CwrResult<()> {
        self.validate(definition_ref, complete_properties)
    }
}
