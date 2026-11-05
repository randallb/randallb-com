use super::*;
pub(super) const METADATA_RELATIONS: &[(&str, &str)] = &[
    ("conversation", "parent"),
    ("conversation", "message"),
    ("conversation", "transfer_metadata"),
];
pub(super) fn reference(selection: &Selection) -> String {
    match selection {
        Selection::Conversation { conversation_ref } => conversation_ref.clone(),
        Selection::ConversationMetadata { conversation_ref } => {
            conversation_ref.clone().unwrap_or_default()
        }
        Selection::Commentary {
            provider_integration_ref,
            response_or_turn_id,
            item_id,
        } => crate::derived_ref(
            "commentary",
            &[provider_integration_ref, response_or_turn_id, item_id],
        ),
        _ => String::new(),
    }
}
pub(super) fn metadata_record_filter(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Conversation => "AND (?4='' OR r.canonical_ref=?4) AND (EXISTS(SELECT 1 FROM edges own WHERE own.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3) AND own.kind='conversation' AND own.target_id=n.ref_id) OR r.claim_owner_ref=r.canonical_ref AND r.claim_kind='node:conversation')",
        RecordKind::Message => "AND EXISTS(SELECT 1 FROM edges own JOIN ref_bindings parent ON parent.ref_id=own.source_id WHERE own.target_id=n.ref_id AND own.kind='message' AND (?4='' OR parent.canonical_ref=?4) AND own.position=(SELECT MIN(first.position) FROM edges first INDEXED BY edges_out WHERE first.source_id=own.source_id AND first.kind='message'))",
        RecordKind::ConversationTransferMetadata => "AND EXISTS(SELECT 1 FROM edges own JOIN ref_bindings parent ON parent.ref_id=own.source_id WHERE own.target_id=n.ref_id AND own.kind='transfer_metadata' AND (?4='' OR parent.canonical_ref=?4))",
        _ => "AND 0",
    }
}
pub(super) fn metadata_relationship_filter(kind: &str) -> &'static str {
    if kind == "message" {
        "AND (?5='' OR source.canonical_ref=?5) AND e.position=(SELECT MIN(first.position) FROM edges first INDEXED BY edges_out WHERE first.source_id=e.source_id AND first.kind='message')"
    } else {
        "AND (?5='' OR source.canonical_ref=?5)"
    }
}

pub(super) fn relationship_filter(
    selection: &Selection,
    source_kind: &str,
    kind: &str,
) -> &'static str {
    if matches!(selection, Selection::ConversationMetadata { .. }) {
        selectors::metadata_relationship_filter(kind)
    } else if matches!(selection, Selection::Commentary { .. }) {
        "AND EXISTS(SELECT 1 FROM edges lookup WHERE lookup.target_id=e.target_id AND lookup.kind='commentary' AND lookup.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?5))"
    } else if matches!(selection, Selection::Conversation { .. }) {
        "AND source.canonical_ref=?5"
    } else if matches!(selection, Selection::Bootstrap) && source_kind == "conversation" {
        "AND (EXISTS(SELECT 1 FROM edges own WHERE own.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?6) AND own.kind='conversation' AND own.target_id=owner.ref_id) OR source.claim_owner_ref=source.canonical_ref AND source.claim_kind='node:conversation')"
    } else if source_kind == "runtime" {
        "AND source.canonical_ref=?6"
    } else {
        ""
    }
}
