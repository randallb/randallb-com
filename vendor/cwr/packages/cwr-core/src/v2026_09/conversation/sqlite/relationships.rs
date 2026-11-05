fn conversation_relationships(
) -> CwrResult<&'static crate::entities::relationships::RelationshipCatalog> {
    static CATALOG: std::sync::OnceLock<
        Result<crate::entities::relationships::RelationshipCatalog, String>,
    > = std::sync::OnceLock::new();
    CATALOG
        .get_or_init(|| {
            crate::entities::relationships::RelationshipCatalog::from_toml(include_str!(
                "../../../../resources/base/conversations/relationships.toml"
            ))
            .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| invalid_error(error.clone()))
}

use cwpdb_store::test_support::retained_relationships::{
    retained_case, RetainedCase, RetainedFacts,
};
fn validate_retained_conversation_relationships(facts: &RetainedFacts) -> CwrResult<()> {
    let catalog = conversation_relationships()?;
    let mut checked = crate::entities::relationships::CheckedRelationshipRefs::default();
    let mut counts = BTreeMap::<(String, String), usize>::new();
    for fact in &facts.relationships {
        if catalog.is_derived(&fact.kind).is_none() {
            continue;
        }
        if fact.schema != crate::cwpdb::EDGE_SCHEMA_REF {
            return invalid(format!("relationship {}: unexpected schema", fact.kind));
        }
        use crate::entities::relationships::EndpointFacts;
        catalog.validate_facts(
            EndpointFacts {
                reference: &fact.source,
                node_kind: fact.source_node.as_deref(),
                edge_kind: fact.source_edge.as_deref(),
            },
            &fact.kind,
            EndpointFacts {
                reference: &fact.target,
                node_kind: fact.target_node.as_deref(),
                edge_kind: fact.target_edge.as_deref(),
            },
            fact.position,
            &fact.value,
            Some(&mut checked),
        )?;
        let count = counts
            .entry((fact.source.clone(), fact.kind.clone()))
            .or_default();
        *count += 1;
        catalog.validate_count(&fact.kind, *count)?;
    }
    Ok(())
}
#[cfg(test)]
mod relationship_tests {
    use super::*;
    #[test]
    fn retained_join_preserves_missing_binding_and_edge_endpoint_checks() {
        use RetainedCase::*;
        for (case, valid) in [
            (MissingMessage, false),
            (ForwardMessage, true),
            (BindingParticipant, true),
            (ParticipantWrongKind, false),
            (ParticipantPerson, true),
            (MessageWrongKind, false),
            (ProjectionEdge, true),
            (MissingProjectionEdge, false),
        ] {
            let facts = retained_case(case).unwrap();
            assert_eq!(
                validate_retained_conversation_relationships(&facts).is_ok(),
                valid,
                "{facts:?}"
            );
        }
    }
    #[test]
    fn historical_invocation_bindings_do_not_relax_canonical_placement_endpoints() {
        use RetainedCase::*;
        let historical = retained_case(HistoricalBinding).unwrap();
        assert!(!historical
            .records
            .iter()
            .any(|n| n.reference == "cwp://example.test/history/invocation"));
        validate_retained_conversation_relationships(&historical).unwrap();
        assert!(validate_retained_conversation_relationships(
            &retained_case(CanonicalInvocationMissing).unwrap()
        )
        .is_err());
        assert!(validate_retained_conversation_relationships(
            &retained_case(HistoricalWrongKind).unwrap()
        )
        .is_err());
    }
    #[test]
    fn owning_persistence_rejects_a_missing_parent_before_commit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.sqlite");
        let mut store =
            ConversationAdmissionStore::open(&path, "cwp://example.test/runtime").unwrap();
        let mut state = store.state.clone();
        state.conversations=vec![serde_json::from_value(json!({"conversation_ref":"cwp://example.test/conversation","participant_actor_refs":["cwp://example.test/person"],"messages":[],"parent_conversation_ref":"cwp://example.test/missing-parent"})).unwrap()].into();
        let error = store.replace_state(state).unwrap_err();
        assert!(error.to_string().contains("missing retained endpoint"));
        assert!(store
            .conversation("cwp://example.test/conversation")
            .is_none());
        drop(store);
        let reopened =
            ConversationAdmissionStore::open(&path, "cwp://example.test/runtime").unwrap();
        assert!(reopened
            .conversation("cwp://example.test/conversation")
            .is_none());
    }
    #[test]
    fn native_contracts_preserve_forward_and_external_refs_but_reject_retained_corruption() {
        let catalog = conversation_relationships().unwrap();
        use crate::entities::relationships::{CheckedRelationshipRefs, EndpointFacts};
        let fact = |reference, node_kind, edge_kind| EndpointFacts {
            reference,
            node_kind,
            edge_kind,
        };
        let conversation = || {
            fact(
                "cwp://example.test/conversation",
                Some("conversation"),
                None,
            )
        };
        let message = || fact("cwp://example.test/message", Some("message"), None);
        let actor = || fact("cwp://example.test/actor", None, None);
        let mut retained = CheckedRelationshipRefs::default();
        catalog
            .validate_facts(conversation(), "participant", actor(), 0, &json!({}), None)
            .unwrap();
        catalog
            .validate_facts(conversation(), "message", actor(), 0, &json!({}), None)
            .unwrap();
        assert!(catalog
            .validate_facts(
                conversation(),
                "message",
                actor(),
                0,
                &json!({}),
                Some(&mut retained)
            )
            .is_err());
        catalog
            .validate_facts(
                conversation(),
                "message",
                message(),
                0,
                &json!({}),
                Some(&mut retained),
            )
            .unwrap();
        assert!(catalog
            .validate_facts(
                conversation(),
                "participant",
                message(),
                1,
                &json!({}),
                None
            )
            .is_err());
        assert!(catalog
            .validate_facts(
                conversation(),
                "conversation_timeline_root",
                actor(),
                1,
                &json!({}),
                None
            )
            .is_err());
        assert!(catalog.validate_count("parent", 2).is_err());
        assert!(catalog
            .validate_facts(
                conversation(),
                "typo_relationship",
                message(),
                0,
                &json!({}),
                None
            )
            .is_err());
        assert!(catalog
            .validate_facts(
                conversation(),
                "timeline_item",
                conversation(),
                0,
                &json!({}),
                None
            )
            .is_err());
        catalog
            .validate_facts(
                conversation(),
                "timeline_item",
                message(),
                0,
                &json!({}),
                None,
            )
            .unwrap();
        let activity = || fact("cwp://example.test/activity", Some("activity"), None);
        catalog
            .validate_facts(
                conversation(),
                "timeline_activity",
                activity(),
                0,
                &json!({}),
                None,
            )
            .unwrap();
        catalog
            .validate_facts(
                conversation(),
                "timeline_item",
                fact("cwp://example.test/edge", None, Some("timeline_activity")),
                0,
                &json!({}),
                Some(&mut retained),
            )
            .unwrap();
        assert_eq!(catalog.is_derived("timeline_item"), Some(true));
    }
}
