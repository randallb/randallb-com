// Bundled native implementation of the normally composed conversations definitions.
// The declarations bind existing record shapes: opening retained data never rewrites it.
fn conversation_entities() -> CwrResult<&'static crate::entities::EntityCatalog> {
    static CATALOG: std::sync::OnceLock<Result<crate::entities::EntityCatalog, String>> =
        std::sync::OnceLock::new();
    CATALOG.get_or_init(|| crate::entities::EntityCatalog::from_toml_documents([
        include_str!("../../../../resources/base/conversations/definitions/conversation.toml"),
        include_str!("../../../../resources/base/conversations/definitions/conversation-message.toml"),
        include_str!("../../../../resources/base/conversations/definitions/conversation-activity.toml"),
        include_str!("../../../../resources/base/conversations/definitions/conversation-transfer-metadata.toml"),
        include_str!("../../../../resources/base/conversations/definitions/conversation-timeline-connection.toml"),
        include_str!("../../../../resources/base/conversations/definitions/conversation-timeline-item.toml"),
        include_str!("../../../../resources/base/conversations/definitions/conversation-participation.toml"),
    ]).map_err(|error| error.to_string())).as_ref().map_err(|error| invalid_error(error.clone()))
}

fn conversation_definition(kind: &str) -> Option<&'static str> {
    match kind {
        "conversation" => Some("cwp://org.coworkerprotocol/cwr/entities/conversation"),
        "message" => Some("cwp://org.coworkerprotocol/cwr/entities/conversation-message"),
        "activity" => Some("cwp://org.coworkerprotocol/cwr/entities/conversation-activity"),
        "conversation_transfer_metadata" => {
            Some("cwp://org.coworkerprotocol/cwr/entities/conversation-transfer-metadata")
        }
        "conversation_timeline_connection" => {
            Some("cwp://org.coworkerprotocol/cwr/entities/conversation-timeline-connection")
        }
        "conversation_timeline_item" => {
            Some("cwp://org.coworkerprotocol/cwr/entities/conversation-timeline-item")
        }
        "participant" => Some("cwp://org.coworkerprotocol/cwr/entities/conversation-participation"),
        _ => None,
    }
}

fn validate_retained_conversation_entities(facts: &RetainedFacts) -> CwrResult<()> {
    validate_retained_conversation_relationships(facts)?;
    let catalog = conversation_entities()?;
    for fact in &facts.records {
        if fact.schema != crate::cwpdb::schema_ref(&fact.kind) {
            return invalid(format!(
                "conversation record {} has an unexpected schema",
                fact.reference
            ));
        }
        catalog
            .validate(
                conversation_definition(&fact.kind).ok_or("unexpected retained record kind")?,
                &fact.value,
            )
            .map_err(|e| invalid_error(format!("conversation record {}: {e}", fact.reference)))?;
    }
    for edge in facts
        .relationships
        .iter()
        .filter(|e| e.kind == "participant")
    {
        catalog.validate(conversation_definition("participant").unwrap(), &edge.value)?;
    }
    Ok(())
}
#[cfg(test)]
mod declared_storage_tests {
    use super::*;
    #[test]
    fn retained_validation_ignores_unowned_collision_nodes_and_rejects_owned_invalid_messages() {
        validate_retained_conversation_entities(
            &retained_case(RetainedCase::UnownedMessage).unwrap(),
        )
        .unwrap();
        let facts = retained_case(RetainedCase::OwnedInvalidMessage).unwrap();
        let error = validate_retained_conversation_entities(&facts).unwrap_err();
        assert!(error.to_string().contains("cwp://example.test/message"));
        assert_eq!(
            facts
                .records
                .iter()
                .find(|n| n.kind == "message")
                .unwrap()
                .value,
            json!({})
        );
    }
}
#[cfg(test)]
#[path = "declared_ownership_tests.rs"]
mod declared_ownership_tests;
