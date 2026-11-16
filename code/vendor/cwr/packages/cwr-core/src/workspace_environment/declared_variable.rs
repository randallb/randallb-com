//! Declared variable operations retain the existing product record format.
use crate::{entities::EntityCatalog, CwrResult};
use serde_json::Value;
use std::sync::OnceLock;

pub(super) const DEFINITION: &str = "cwp://org.coworkerprotocol/wlp/entities/workspace-variable";

fn catalog() -> CwrResult<&'static EntityCatalog> {
    static CATALOG: OnceLock<Result<EntityCatalog, String>> = OnceLock::new();
    let catalog = CATALOG.get_or_init(|| {
        EntityCatalog::from_toml(include_str!(
            "../../resources/base/entities/workspace-variable.toml"
        ))
        .map_err(|e| e.to_string())
    });
    Ok(catalog.as_ref().map_err(|e| e.clone())?)
}

pub(super) fn validate(value: &Value) -> CwrResult<()> {
    catalog()?.validate(DEFINITION, value)
}
