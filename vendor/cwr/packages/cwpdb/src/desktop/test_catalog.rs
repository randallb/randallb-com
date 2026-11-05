use crate::{
    declared::{Catalog, EntityContract, ValueValidator},
    CwpdbResult,
};
use std::{collections::BTreeMap, sync::Arc};
struct Validator(BTreeMap<String, jsonschema::Validator>);
impl ValueValidator for Validator {
    fn validate_reference(&self, reference: &str) -> CwpdbResult<()> {
        if reference.starts_with("cwp://") && !reference.contains('#') {
            Ok(())
        } else {
            Err("invalid fixture reference".into())
        }
    }
    fn validate(&self, definition: &str, value: &serde_json::Value) -> CwpdbResult<()> {
        if self
            .0
            .get(definition)
            .ok_or("unknown fixture definition")?
            .is_valid(value)
        {
            Ok(())
        } else {
            Err("entity value does not satisfy its definition".into())
        }
    }
}
pub(crate) fn catalog() -> CwpdbResult<Catalog> {
    from_documents([
        include_str!("fixtures/personal-collection.toml"),
        include_str!("fixtures/personal-collection-scope.toml"),
        include_str!("fixtures/sidebar-pin-target.toml"),
        include_str!("fixtures/sidebar-pin-scope.toml"),
        include_str!("fixtures/local-usage-cursor.toml"),
        include_str!("fixtures/local-usage-event.toml"),
        include_str!("fixtures/posthog-state.toml"),
        include_str!("fixtures/posthog-event.toml"),
        include_str!("fixtures/desktop-workspace.toml"),
        include_str!("fixtures/desktop-workspace-binding.toml"),
        include_str!("fixtures/desktop-registry-receipt.toml"),
        include_str!("fixtures/desktop-instance.toml"),
        include_str!("fixtures/desktop-version.toml"),
        include_str!("fixtures/desktop-folder.toml"),
        include_str!("fixtures/desktop-version-membership.toml"),
        include_str!("fixtures/desktop-import.toml"),
    ])
}

pub(crate) fn from_documents<'a>(
    sources: impl IntoIterator<Item = &'a str>,
) -> CwpdbResult<Catalog> {
    let mut validators = BTreeMap::new();
    let mut contracts = Vec::new();
    for source in sources {
        let document: serde_json::Value =
            serde_json::to_value(toml::from_str::<toml::Value>(source)?)?;
        let definition = document["ref"]
            .as_str()
            .ok_or("missing fixture ref")?
            .to_owned();
        let validator = jsonschema::options()
            .with_format("cwr-ref", |s: &str| s.starts_with("cwp://"))
            .should_validate_formats(true)
            .build(&document["value_schema"])?;
        validators.insert(definition.clone(), validator);
        contracts.push(EntityContract {
            definition_ref: definition.clone(),
            kind: document["record_kind"]
                .as_str()
                .unwrap_or("declared_entity")
                .to_owned(),
            schema_ref: document["record_schema"]
                .as_str()
                .unwrap_or(&definition)
                .to_owned(),
            disposable: document["lifetime"].as_str() == Some("disposable"),
        });
    }
    Catalog::new(contracts, Arc::new(Validator(validators)))
}

pub(crate) fn entity_fixture(source: &str) -> CwpdbResult<crate::declared::EntityBinding> {
    let document: serde_json::Value = serde_json::to_value(toml::from_str::<toml::Value>(source)?)?;
    from_documents([source])?.entity(document["ref"].as_str().ok_or("missing fixture ref")?)
}
