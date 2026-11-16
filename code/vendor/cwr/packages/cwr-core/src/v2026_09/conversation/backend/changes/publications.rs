use super::*;
pub(super) fn publication_changes(
    result: &mut Changes,
    state: &ConversationRuntimeState,
) -> CwrResult<()> {
    // Determine affected public roots from the journal, including changes to
    // direct execution conversations and invocation ancestry.
    let mut roots = std::collections::BTreeSet::new();
    for (_, old, new) in state.conversations.changes() {
        if old == Some(new) {
            continue;
        }
        for conversation in old.into_iter().chain(std::iter::once(new)) {
            roots.insert(
                conversation
                    .parent_conversation_ref
                    .as_ref()
                    .unwrap_or(&conversation.conversation_ref)
                    .clone(),
            );
        }
    }
    for (_, old, new) in state.invocation_ledger.changes() {
        if old != new {
            roots.extend(
                old.into_iter()
                    .chain(new)
                    .filter_map(|record| record.conversation_ref.clone()),
            );
        }
    }
    for (_, old, new) in state.invocation_execution_conversation_refs.changes() {
        if old != new {
            for reference in old.into_iter().chain(new) {
                if let Some(conversation) = state
                    .conversations
                    .iter()
                    .find(|conversation| &conversation.conversation_ref == reference)
                {
                    roots.insert(
                        conversation
                            .parent_conversation_ref
                            .as_ref()
                            .unwrap_or(reference)
                            .clone(),
                    );
                }
            }
        }
    }
    for (_, old, new) in state.invocation_ledger.changed_waits() {
        if old != Some(new) {
            for wait in old.into_iter().chain(std::iter::once(new)) {
                for reference in [&wait.waiter_invocation_ref, &wait.awaited_invocation_ref] {
                    if let Some(root) = state
                        .invocation_ledger
                        .invocation(reference)
                        .and_then(|record| record.conversation_ref.clone())
                    {
                        roots.insert(root);
                    }
                }
            }
        }
    }
    for reference in roots {
        let Some(root) = state.conversations.iter().find(|conversation| {
            conversation.conversation_ref == reference
                && conversation.parent_conversation_ref.is_none()
        }) else {
            continue;
        };
        result
            .publications
            .push(projected_publication(state, root)?);
    }
    Ok(())
}

pub(super) fn projected_publication(
    state: &ConversationRuntimeState,
    root: &RetainedConversation,
) -> CwrResult<PublicationSuffix> {
    let projected = projection::authoritative_projection(state, root);
    let positions = projected
        .messages
        .iter()
        .enumerate()
        .map(|(position, message)| (message.message_ref.as_str(), position as u64))
        .collect::<BTreeMap<_, _>>();
    let items = projection::semantic_timeline_items(&projected)
        .into_iter()
        .enumerate()
        .map(|(position, (kind, reference))| {
            if kind == "message" {
                PublishedItem::Message {
                    position: *positions
                        .get(reference.as_str())
                        .expect("semantic message belongs to projected conversation"),
                    message_ref: reference,
                }
            } else {
                PublishedItem::Turn {
                    invocation_ref: reference,
                    position: position as u64,
                }
            }
        })
        .collect();
    let activities = projected
        .activity
        .iter()
        .enumerate()
        .filter(|(_, activity)| {
            activity_kind(activity).0 == RecordKind::Activity
                && !matches!(activity.event, ConversationActivityEvent::WorkPhase { .. })
        })
        .map(|(position, activity)| {
            Ok(PublishedActivity {
                activity_ref: activity.activity_ref.clone(),
                position: position as u64,
                record: serde_json::to_value(activity)?,
            })
        })
        .collect::<CwrResult<Vec<_>>>()?;
    Ok(PublicationSuffix {
        conversation_ref: root.conversation_ref.clone(),
        from_position: 0,
        items,
        activities,
        remove_activities: Vec::new(),
    })
}
