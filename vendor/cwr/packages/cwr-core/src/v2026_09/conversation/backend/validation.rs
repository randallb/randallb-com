//! CWR relationship contracts validate canonical facts on either backend.
use super::*;
use crate::entities::relationships::{CheckedRelationshipRefs, EndpointFacts, RelationshipCatalog};
fn catalog() -> CwrResult<&'static RelationshipCatalog> {
    static CATALOG: std::sync::OnceLock<Result<RelationshipCatalog, String>> =
        std::sync::OnceLock::new();
    CATALOG
        .get_or_init(|| {
            RelationshipCatalog::from_toml(include_str!(
                "../../../../resources/base/conversations/relationships.toml"
            ))
            .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| invalid_error(error.clone()))
}
impl ConversationBackend {
    fn fact_kind(
        &mut self,
        records: &BTreeMap<&str, RecordKind>,
        cache: &mut BTreeMap<String, EndpointKinds>,
        reference: &str,
    ) -> CwrResult<EndpointKinds> {
        if let Some(kind) = records.get(reference) {
            return Ok(EndpointKinds {
                node_kind: Some(kind.as_str().into()),
                edge_kind: None,
            });
        }
        if let Some(kind) = cache.get(reference) {
            return Ok(kind.clone());
        }
        let kind = self
            .storage
            .endpoint_kinds(&self.scope, reference, self.authorizer.as_ref())?;
        cache.insert(reference.into(), kind.clone());
        Ok(kind)
    }
    pub(super) fn validate_snapshot(&mut self, page: &SnapshotPage) -> CwrResult<()> {
        let records = page
            .records
            .iter()
            .map(|record| (record.reference.as_str(), record.kind))
            .collect::<BTreeMap<_, _>>();
        let mut checked = CheckedRelationshipRefs::default();
        let mut cache = BTreeMap::new();
        let mut counts = BTreeMap::new();
        for edge in &page.relationships {
            if catalog()?.is_derived(&edge.kind).is_none() {
                continue;
            }
            let source = self.fact_kind(&records, &mut cache, &edge.source_ref)?;
            let target = self.fact_kind(&records, &mut cache, &edge.target_ref)?;
            catalog()?.validate_facts(
                EndpointFacts {
                    reference: &edge.source_ref,
                    node_kind: source.node_kind.as_deref(),
                    edge_kind: source.edge_kind.as_deref(),
                },
                &edge.kind,
                EndpointFacts {
                    reference: &edge.target_ref,
                    node_kind: target.node_kind.as_deref(),
                    edge_kind: target.edge_kind.as_deref(),
                },
                edge.position,
                &edge.properties,
                Some(&mut checked),
            )?;
            if catalog()?.singleton(&edge.kind) {
                *counts.entry((&edge.source_ref, &edge.kind)).or_insert(0) += 1;
            }
        }
        for ((_, kind), count) in counts {
            catalog()?.validate_count(kind, count)?;
        }
        Ok(())
    }
    pub(super) fn validate_changes(&mut self, changes: &Changes) -> CwrResult<()> {
        let records = changes
            .records
            .iter()
            .map(|record| (record.reference.as_str(), record.kind))
            .chain(
                changes
                    .timeline_items
                    .iter()
                    .map(|item| (item.item_ref.as_str(), RecordKind::ConversationTimelineItem)),
            )
            .collect::<BTreeMap<_, _>>();
        let mut cache = BTreeMap::new();
        let timeline_memberships = changes
            .timeline_items
            .iter()
            .flat_map(|item| item.memberships())
            .collect::<Vec<_>>();
        for change in changes.memberships.iter().chain(&timeline_memberships) {
            let (source, kind, members) = match change {
                MembershipChange::Upsert {
                    source_ref,
                    kind,
                    member,
                } => (source_ref, *kind, std::slice::from_ref(member)),
                MembershipChange::Replace { membership } => (
                    &membership.source_ref,
                    membership.kind,
                    membership.members.as_slice(),
                ),
                MembershipChange::Remove { .. } => continue,
            };
            if catalog()?.is_derived(kind.as_str()).is_none() {
                continue;
            }
            if matches!(change, MembershipChange::Replace { .. }) {
                catalog()?.validate_count(kind.as_str(), members.len())?;
            }
            let source_kind = self.fact_kind(&records, &mut cache, source)?;
            for member in members {
                let target_kind = self.fact_kind(&records, &mut cache, &member.target_ref)?;
                catalog()?.validate_facts(
                    EndpointFacts {
                        reference: source,
                        node_kind: source_kind.node_kind.as_deref(),
                        edge_kind: source_kind.edge_kind.as_deref(),
                    },
                    kind.as_str(),
                    EndpointFacts {
                        reference: &member.target_ref,
                        node_kind: target_kind.node_kind.as_deref(),
                        edge_kind: target_kind.edge_kind.as_deref(),
                    },
                    member.position,
                    &member.properties,
                    None,
                )?;
            }
        }
        Ok(())
    }
}
