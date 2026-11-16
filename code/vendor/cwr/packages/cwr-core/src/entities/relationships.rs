//! Declaration checks for domain-owned relationships. This catalog grants no
//! authority and does not change graph identity, ordering, or transaction rules.
use crate::{CwrRef, CwrResult};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

pub const DEFINITION_TYPE: &str =
    "cwp://org.coworkerprotocol/2026-10-23/schemas/relationship-catalog";

pub struct RelationshipCatalog(BTreeMap<String, Definition>);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    #[serde(rename = "type")]
    type_ref: String,
    #[serde(rename = "ref")]
    reference: String,
    relationships: Vec<Definition>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    kind: String,
    source: Vec<String>,
    target: Vec<String>,
    singleton: bool,
    ordered: bool,
    derived: bool,
    value_schema: Value,
    #[serde(skip)]
    validator: Option<jsonschema::Validator>,
}
pub(crate) struct EndpointFacts<'a> {
    pub reference: &'a str,
    pub node_kind: Option<&'a str>,
    pub edge_kind: Option<&'a str>,
}

/// Lives for one retained scan; endpoint kinds and existence are never cached.
#[derive(Default)]
pub(crate) struct CheckedRelationshipRefs(HashSet<String>);
impl CheckedRelationshipRefs {
    fn check(&mut self, reference: &str) -> CwrResult<()> {
        if !self.0.contains(reference) {
            CwrRef::parse(reference)?;
            self.0.insert(reference.to_owned());
        }
        Ok(())
    }
}

impl RelationshipCatalog {
    pub fn from_toml(source: &str) -> CwrResult<Self> {
        let document: Document = toml::from_str(source)?;
        if document.type_ref != DEFINITION_TYPE {
            return Err("unsupported relationship declaration".into());
        }
        CwrRef::parse(&document.reference)?;
        let mut definitions = BTreeMap::new();
        for mut definition in document.relationships {
            if definition.kind.is_empty()
                || definition.source.is_empty()
                || definition.target.is_empty()
            {
                return Err("relationship requires kind and endpoints".into());
            }
            for endpoint in definition.source.iter().chain(&definition.target) {
                if !matches!(
                    endpoint.as_str(),
                    "runtime"
                        | "schedule_owner"
                        | "installed_schedule"
                        | "schedule_occurrence"
                        | "conversation"
                        | "message"
                        | "activity"
                        | "invocation"
                        | "conversation_transfer_metadata"
                        | "conversation_timeline_connection"
                        | "conversation_timeline_item"
                        | "external:actor"
                        | "binding:activity"
                        | "binding:invocation"
                        | "binding:projection_item"
                        | "edge:timeline_activity"
                ) {
                    return Err(format!("unsupported relationship endpoint {endpoint}").into());
                }
            }
            if definition.value_schema.get("type").and_then(Value::as_str) != Some("object") {
                return Err("relationship value schema must require an object".into());
            }
            definition.validator = Some(jsonschema::validator_for(&definition.value_schema)?);
            if definitions
                .insert(definition.kind.clone(), definition)
                .is_some()
            {
                return Err("duplicate relationship kind".into());
            }
        }
        Ok(Self(definitions))
    }

    fn definition(&self, kind: &str) -> CwrResult<Option<&Definition>> {
        let Some(definition) = self.0.get(kind) else {
            if kind.starts_with("field:")
                || matches!(
                    kind,
                    "wait"
                        | "execution"
                        | "tool_attempt"
                        | "tool_operation"
                        | "tool_file"
                        | "child_invocation"
                        | "commentary"
                )
            {
                return Ok(None);
            }
            return Err(format!("undeclared conversation relationship {kind}").into());
        };
        Ok(Some(definition))
    }

    /// Missing endpoint nodes are intentional: domain writes establish links
    /// before records, and actors can be binding-only. Validate known types now;
    /// the owning domain retains its existing closure/lifecycle checks.
    /// Explicit binding:invocation contracts preserve historical identity without
    /// granting execution authority; plain invocation still requires a record.
    /// Retained and write paths supply endpoint facts from their declared storage snapshot.
    pub(crate) fn validate_facts(
        &self,
        source: EndpointFacts<'_>,
        kind: &str,
        target: EndpointFacts<'_>,
        position: i64,
        value: &Value,
        mut retained_refs: Option<&mut CheckedRelationshipRefs>,
    ) -> CwrResult<()> {
        let Some(definition) = self.definition(kind)? else {
            return Ok(());
        };
        if position < 0 || (!definition.ordered && position != 0) {
            return Err(format!("relationship {kind}: invalid position").into());
        }
        if !definition.validator.as_ref().unwrap().is_valid(value) {
            return Err(format!("relationship {kind}: invalid value").into());
        }
        for (facts, kinds) in [(source, &definition.source), (target, &definition.target)] {
            let reference = facts.reference;
            if let Some(checked) = retained_refs.as_deref_mut() {
                checked.check(reference)?;
            } else {
                CwrRef::parse(reference)?;
            }
            if let Some(actual) = facts.node_kind {
                if !kinds.iter().any(|kind| {
                    kind == actual
                        || (kind == "binding:activity" && actual == "activity")
                        || (kind == "binding:invocation" && actual == "invocation")
                        || (kind == "external:actor"
                            && matches!(actual, "actor" | "coworker" | "person"))
                }) {
                    return Err(format!(
                        "relationship {kind}: unexpected endpoint kind {actual} at {reference}"
                    )
                    .into());
                }
            }
            if retained_refs.is_some() {
                Self::check_presence(
                    kind,
                    kinds,
                    reference,
                    facts.node_kind.is_some(),
                    facts.edge_kind,
                )?;
            }
        }
        Ok(())
    }

    fn check_presence(
        kind: &str,
        kinds: &[String],
        reference: &str,
        node_present: bool,
        edge_kind: Option<&str>,
    ) -> CwrResult<()> {
        if node_present
            || kinds.iter().any(|kind| {
                matches!(
                    kind.as_str(),
                    "external:actor"
                        | "binding:activity"
                        | "binding:projection_item"
                        | "binding:invocation"
                ) || (kind == "edge:timeline_activity" && edge_kind == Some("timeline_activity"))
            })
        {
            return Ok(());
        }
        Err(format!("relationship {kind}: missing retained endpoint {reference}").into())
    }

    pub(crate) fn singleton(&self, kind: &str) -> bool {
        self.0
            .get(kind)
            .is_some_and(|definition| definition.singleton)
    }

    pub(crate) fn validate_count(&self, kind: &str, count: usize) -> CwrResult<()> {
        if self
            .0
            .get(kind)
            .is_some_and(|definition| definition.singleton && count > 1)
        {
            return Err(format!("relationship {kind}: more than one target").into());
        }
        Ok(())
    }

    pub fn is_derived(&self, kind: &str) -> Option<bool> {
        self.0.get(kind).map(|definition| definition.derived)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document() -> String {
        r#"type = "cwp://org.coworkerprotocol/2026-10-23/schemas/relationship-catalog"
ref = "cwp://example.test/relationships"
[[relationships]]
kind = "example"
source = ["conversation"]
target = ["message"]
singleton = false
ordered = true
derived = false
value_schema = { type = "object" }
"#
        .to_owned()
    }
    #[test]
    fn checked_refs_only_retain_successful_parses_and_ignored_links_need_no_sql() {
        let mut checked = CheckedRelationshipRefs::default();
        checked.check("cwp://example.test/record").unwrap();
        checked.check("cwp://example.test/record").unwrap();
        assert_eq!(checked.0.len(), 1);
        for _ in 0..2 {
            assert!(checked.check("invalid reference").is_err());
            assert_eq!(checked.0.len(), 1);
        }
        let catalog = RelationshipCatalog::from_toml(&document()).unwrap();
        // Ignored relationships do not inspect or parse endpoint facts.
        catalog
            .validate_facts(
                EndpointFacts {
                    reference: "ignored",
                    node_kind: None,
                    edge_kind: None,
                },
                "field:record",
                EndpointFacts {
                    reference: "ignored",
                    node_kind: None,
                    edge_kind: None,
                },
                0,
                &serde_json::json!({}),
                None,
            )
            .unwrap();
        let error = catalog
            .validate_facts(
                EndpointFacts {
                    reference: "ignored",
                    node_kind: None,
                    edge_kind: None,
                },
                "unknown",
                EndpointFacts {
                    reference: "ignored",
                    node_kind: None,
                    edge_kind: None,
                },
                0,
                &serde_json::json!({}),
                None,
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("undeclared conversation relationship"));
    }

    #[test]
    fn declaration_parser_rejects_duplicates_unknown_shapes_and_invalid_schemas() {
        let source = document();
        RelationshipCatalog::from_toml(&source).unwrap();
        let edge = source.split("[[relationships]]").nth(1).unwrap();
        assert!(
            RelationshipCatalog::from_toml(&format!("{source}\n[[relationships]]{edge}")).is_err()
        );
        for invalid in [
            source.replace("[\"message\"]", "[\"mesage\"]"),
            source.replace("ordered = true", "ordered = \"descending\""),
            source.replace("ordered = true", "ordered = true\nunknown = true"),
            source.replace("type = \"object\"", "type = \"unknown\""),
            source.replace("type = \"object\"", "type = \"string\""),
        ] {
            assert!(RelationshipCatalog::from_toml(&invalid).is_err());
        }
    }
    #[test]
    fn retained_relationship_facts_preserve_closure_and_binding_exceptions() {
        fn validate(
            target_kind: &str,
            node: Option<&str>,
            edge: Option<&str>,
            present: bool,
        ) -> CwrResult<()> {
            let source = document().replace("[\"message\"]", &format!("[\"{target_kind}\"]"));
            let catalog = RelationshipCatalog::from_toml(&source)?;
            catalog.validate_facts(
                EndpointFacts {
                    reference: "cwp://example.test/conversation",
                    node_kind: present.then_some("conversation"),
                    edge_kind: None,
                },
                "example",
                EndpointFacts {
                    reference: "cwp://example.test/target",
                    node_kind: node,
                    edge_kind: edge,
                },
                0,
                &serde_json::json!({}),
                Some(&mut CheckedRelationshipRefs::default()),
            )
        }
        validate("message", Some("message"), None, true).unwrap();
        assert!(validate("message", None, None, true)
            .unwrap_err()
            .to_string()
            .contains("missing retained endpoint"));
        assert!(validate("message", Some("invocation"), None, true)
            .unwrap_err()
            .to_string()
            .contains("unexpected endpoint kind"));
        assert!(validate("message", Some("message"), None, false).is_err());
        validate("binding:invocation", None, None, true).unwrap();
        validate("external:actor", None, None, true).unwrap();
        validate("external:actor", Some("person"), None, true).unwrap();
        validate(
            "edge:timeline_activity",
            None,
            Some("timeline_activity"),
            true,
        )
        .unwrap();
        assert!(validate("edge:timeline_activity", None, Some("other"), true).is_err());
    }
}
