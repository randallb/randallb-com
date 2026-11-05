use super::*;
pub(super) fn timeline_changes(
    result: &mut Changes,
    state: &ConversationRuntimeState,
) -> CwrResult<()> {
    for (conversation, old, new) in state.timeline_store.changes() {
        if old == new {
            continue;
        }
        let new = new.ok_or("retained timelines are append-only")?;
        if old.is_none() {
            member(
                result,
                conversation,
                MembershipKind::ConversationTimelineRoot,
                &new.root_connection_ref,
                0,
                json!({}),
            )?;
        }
        for (index, connection) in new.connections.iter().enumerate() {
            let previous = old.and_then(|old| old.connections.get(index));
            if previous.is_none_or(|old| {
                old.connection_ref != connection.connection_ref
                    || old.owner != connection.owner
                    || old.revision != connection.revision
                    || old.next_sequence != connection.next_sequence
            }) {
                properties(
                    result,
                    &connection.connection_ref,
                    RecordKind::ConversationTimelineConnection,
                    json!({"revision":connection.revision,"next_sequence":connection.next_sequence,"store_sequence":index}),
                    Some(conversation),
                );
            }
            if let Some(previous) = previous {
                for item in &previous.items {
                    if !connection
                        .items
                        .iter()
                        .any(|new| new.item_ref == item.item_ref)
                    {
                        remove(result, &item.item_ref, RecordKind::ConversationTimelineItem);
                    }
                }
            }
            for item in &connection.items {
                if previous.is_some_and(|old| old.items.iter().any(|old| old == item)) {
                    continue;
                }
                result
                    .timeline_items
                    .push(crate::cwpdb::conversation::TimelineItemWrite {
                        conversation_ref: conversation.into(),
                        connection_ref: item.connection_ref.clone(),
                        item_ref: item.item_ref.clone(),
                        publication_ref: item.publication_ref.clone(),
                        position: item.sibling_sequence.try_into()?,
                        content: match &item.content {
                            ConversationTimelineContent::Message { message_ref } => {
                                crate::cwpdb::conversation::TimelineContent::Message {
                                    message_ref: message_ref.clone(),
                                }
                            }
                            ConversationTimelineContent::Invocation { invocation_ref } => {
                                crate::cwpdb::conversation::TimelineContent::Invocation {
                                    invocation_ref: invocation_ref.clone(),
                                }
                            }
                        },
                        child_connection_ref: item.child_connection_ref.clone(),
                    });
            }
        }
    }
    Ok(())
}
