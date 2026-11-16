use super::*;

pub(crate) fn validate_retained_timelines(state: &ConversationRuntimeState) -> CwrResult<()> {
    let mut connection_refs = BTreeSet::new();
    let mut item_refs = BTreeSet::new();
    let mut publication_refs = BTreeSet::new();
    let mut content_refs = BTreeSet::new();
    let conversation_refs = state
        .conversations
        .iter()
        .map(|conversation| conversation.conversation_ref.as_str())
        .collect::<BTreeSet<_>>();
    for conversation in &state.conversations {
        content_refs.extend(
            conversation
                .messages
                .iter()
                .map(|message| message.message_ref.as_str()),
        );
    }
    content_refs.extend(
        state
            .invocation_ledger
            .invocations()
            .map(|invocation| invocation.invocation_ref.as_str()),
    );

    for conversation in &state.conversations {
        match &conversation.timeline_connection_ref {
            Some(root_connection_ref) => {
                if conversation.parent_conversation_ref.is_some() {
                    return invalid(
                        "private execution conversations cannot retain a shared timeline",
                    );
                }
                let timeline = state
                    .timeline_store
                    .timeline(&conversation.conversation_ref)
                    .ok_or_else(|| {
                        invalid_error(format!(
                            "conversation {} names a missing timeline",
                            conversation.conversation_ref
                        ))
                    })?;
                if &timeline.root_connection_ref != root_connection_ref {
                    return invalid(format!(
                        "conversation {} names the wrong timeline root",
                        conversation.conversation_ref
                    ));
                }
            }
            None if state
                .timeline_store
                .timeline(&conversation.conversation_ref)
                .is_some() =>
            {
                return invalid(format!(
                    "legacy conversation {} has a retained timeline",
                    conversation.conversation_ref
                ));
            }
            None => {}
        }
    }

    for (conversation_ref, timeline) in state.timeline_store.iter() {
        let conversation = state
            .conversations
            .iter()
            .find(|conversation| &conversation.conversation_ref == conversation_ref)
            .ok_or_else(|| {
                invalid_error(format!(
                    "timeline belongs to missing conversation {conversation_ref}"
                ))
            })?;
        timeline.validate_for_conversation(conversation_ref)?;
        if conversation.timeline_connection_ref.as_deref()
            != Some(timeline.root_connection_ref.as_str())
        {
            return invalid(format!(
                "timeline for {conversation_ref} is not the conversation's retained root"
            ));
        }
        for connection in &timeline.connections {
            if !connection_refs.insert(connection.connection_ref.as_str()) {
                return invalid(format!(
                    "retained timelines repeat connection {}",
                    connection.connection_ref
                ));
            }
            if connection.revision > i64::MAX as u64 || connection.next_sequence > i64::MAX as u64 {
                return invalid("timeline counters exceed the CWPDB integer range");
            }
            for item in &connection.items {
                if item.sibling_sequence > i64::MAX as u64 {
                    return invalid("timeline sequence exceeds the CWPDB integer range");
                }
                if !item_refs.insert(item.item_ref.as_str()) {
                    return invalid(format!("retained timelines repeat item {}", item.item_ref));
                }
                if !publication_refs.insert(item.publication_ref.as_str()) {
                    return invalid(format!(
                        "retained timelines repeat publication {}",
                        item.publication_ref
                    ));
                }
                if conversation_refs.contains(item.content.reference()) {
                    return invalid(format!(
                        "timeline item {} reuses a conversation as content",
                        item.item_ref
                    ));
                }
                content_refs.insert(item.content.reference());
                validate_timeline_content(state, conversation, timeline, item)?;
            }
        }
    }

    if connection_refs
        .iter()
        .any(|reference| item_refs.contains(reference) || publication_refs.contains(reference))
        || item_refs
            .iter()
            .any(|reference| publication_refs.contains(reference))
        || connection_refs
            .iter()
            .chain(&item_refs)
            .chain(&publication_refs)
            .any(|reference| {
                content_refs.contains(reference) || conversation_refs.contains(reference)
            })
    {
        return invalid(
            "retained timelines conflate connection, placement, publication, or content identity",
        );
    }

    validate_invocation_publications(
        state,
        &connection_refs,
        &item_refs,
        &mut publication_refs,
        &content_refs,
        &conversation_refs,
    )?;
    Ok(())
}

fn validate_invocation_publications<'a>(
    state: &'a ConversationRuntimeState,
    connection_refs: &BTreeSet<&'a str>,
    item_refs: &BTreeSet<&'a str>,
    publication_refs: &mut BTreeSet<&'a str>,
    content_refs: &BTreeSet<&'a str>,
    conversation_refs: &BTreeSet<&'a str>,
) -> CwrResult<()> {
    for invocation in state.invocation_ledger.invocations() {
        if invocation.timeline_publications.is_empty() {
            continue;
        }
        let conversation_ref = invocation.conversation_ref.as_deref().ok_or_else(|| {
            invalid_error(format!(
                "invocation {} has a timeline decision without a conversation",
                invocation.invocation_ref
            ))
        })?;
        let conversation = state
            .conversations
            .iter()
            .find(|conversation| conversation.conversation_ref == conversation_ref)
            .ok_or_else(|| {
                invalid_error(format!(
                    "invocation {} names a missing conversation",
                    invocation.invocation_ref
                ))
            })?;
        // Disclosed fork history can outlive an undisclosed causal message.
        // Executable records still require the complete source relationship.
        if invocation.forked_from_invocation_ref.is_none()
            || invocation.source_message_ref.is_some()
        {
            let retained_source =
                retained_invocation_source_message_ref(state, &invocation.invocation_ref)?;
            if invocation.source_message_ref.as_deref() != Some(retained_source) {
                return invalid(format!(
                    "invocation {} does not name its retained causal source",
                    invocation.invocation_ref
                ));
            }
            if !conversation
                .messages
                .iter()
                .any(|message| message.message_ref == retained_source)
            {
                return invalid(format!(
                    "invocation {} names a source message outside its conversation",
                    invocation.invocation_ref
                ));
            }
        }
        let timeline = state
            .timeline_store
            .timeline(conversation_ref)
            .ok_or_else(|| {
                invalid_error(format!(
                    "invocation {} has a timeline decision without a retained timeline",
                    invocation.invocation_ref
                ))
            })?;
        for (publication_ref, disposition) in &invocation.timeline_publications {
            let placed = timeline.find_publication(publication_ref);
            match (disposition, placed) {
                (ConversationTimelineDisposition::Undisclosed, None) => {
                    if !publication_refs.insert(publication_ref.as_str())
                        || connection_refs.contains(publication_ref.as_str())
                        || item_refs.contains(publication_ref.as_str())
                        || content_refs.contains(publication_ref.as_str())
                        || conversation_refs.contains(publication_ref.as_str())
                    {
                        return invalid(format!(
                            "invocation {} reuses a retained timeline identity",
                            invocation.invocation_ref
                        ));
                    }
                }
                (ConversationTimelineDisposition::Undisclosed, Some(_)) => {
                    return invalid(format!(
                        "undisclosed invocation {} has a timeline item",
                        invocation.invocation_ref
                    ));
                }
                (_, Some(item))
                    if item.content
                        == (ConversationTimelineContent::Invocation {
                            invocation_ref: invocation.invocation_ref.clone(),
                        })
                        && timeline.disposition_for(item) == *disposition => {}
                _ => {
                    return invalid(format!(
                        "invocation {} has no matching timeline item",
                        invocation.invocation_ref
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_changed_timelines(state: &ConversationRuntimeState) -> CwrResult<()> {
    validate_new_content_identities(state)?;
    let timeline_changed = state
        .timeline_store
        .changes()
        .any(|(_, old, new)| old != new);
    let conversation_timeline_changed =
        state
            .conversations
            .changes()
            .any(|(_, old, conversation)| match old {
                None => conversation.timeline_connection_ref.is_some(),
                Some(old) => {
                    old.timeline_connection_ref != conversation.timeline_connection_ref
                        || (conversation.timeline_connection_ref.is_some()
                            && (old.parent_conversation_ref
                                != conversation.parent_conversation_ref
                                || !message_refs_are_preserved(old, conversation)))
                }
            });
    let invocation_timeline_changed = state.invocation_ledger.changes().any(|(_, old, new)| {
        let Some(new) = new else {
            return old.is_some_and(|old| !old.timeline_publications.is_empty());
        };
        match old {
            None => !new.timeline_publications.is_empty(),
            Some(old) => {
                old.conversation_ref != new.conversation_ref
                    && (!old.timeline_publications.is_empty()
                        || !new.timeline_publications.is_empty())
                    || old.source_message_ref != new.source_message_ref
                    || old.timeline_publications != new.timeline_publications
            }
        }
    });
    if !timeline_changed && !conversation_timeline_changed && !invocation_timeline_changed {
        return Ok(());
    }
    validate_retained_timelines(state)?;
    for (_, old, new) in state.timeline_store.changes() {
        match (old, new) {
            (None, Some(_)) => {}
            (Some(old), Some(new)) => validate_append_only(
                old,
                new,
                &state
                    .streaming_messages
                    .changes()
                    .filter_map(|(reference, old, new)| {
                        (old.is_some() && new.is_none()).then_some(reference.as_str())
                    })
                    .collect(),
            )?,
            (Some(_), None) => return invalid("retained timelines are append-only"),
            (None, None) => {}
        }
    }
    for (_, old, conversation) in state.conversations.changes() {
        if old.is_some_and(|old| {
            old.timeline_connection_ref.is_some()
                && old.timeline_connection_ref != conversation.timeline_connection_ref
        }) {
            return invalid("a retained conversation timeline root cannot change");
        }
    }
    Ok(())
}

fn validate_new_content_identities(state: &ConversationRuntimeState) -> CwrResult<()> {
    if state.timeline_store.is_empty() {
        return Ok(());
    }
    let mut new_content_refs = Vec::new();
    let mut new_conversation_refs = Vec::new();
    for (_, old, conversation) in state.conversations.changes() {
        if old.is_none() {
            new_conversation_refs.push(conversation.conversation_ref.as_str());
        }
        for message in &conversation.messages {
            if old.is_none_or(|old| {
                !old.messages
                    .iter()
                    .any(|old| old.message_ref == message.message_ref)
            }) {
                new_content_refs.push(("message", message.message_ref.as_str()));
            }
        }
    }
    for (_, old, new) in state.invocation_ledger.changes() {
        if old.is_none() {
            if let Some(invocation) = new {
                new_content_refs.push(("invocation", invocation.invocation_ref.as_str()));
            }
        }
    }
    if new_content_refs.is_empty() && new_conversation_refs.is_empty() {
        return Ok(());
    }

    let mut timeline_identity_refs = BTreeSet::new();
    for (_, timeline) in state.timeline_store.iter() {
        for connection in &timeline.connections {
            timeline_identity_refs.insert(connection.connection_ref.as_str());
            for item in &connection.items {
                timeline_identity_refs.insert(item.item_ref.as_str());
                timeline_identity_refs.insert(item.publication_ref.as_str());
            }
        }
    }
    for invocation in state.invocation_ledger.invocations() {
        timeline_identity_refs.extend(invocation.timeline_publications.keys().map(String::as_str));
    }

    for reference in new_conversation_refs {
        let reused_by_timeline = state
            .timeline_store
            .iter()
            .filter(|(conversation_ref, _)| conversation_ref.as_str() != reference)
            .any(|(_, timeline)| timeline.identity_in_use(reference));
        if reused_by_timeline || timeline_identity_refs.contains(reference) {
            return invalid(format!(
                "new conversation {reference} reuses a retained timeline identity"
            ));
        }
    }

    for (kind, reference) in new_content_refs {
        if timeline_identity_refs.contains(reference) {
            return invalid(format!(
                "new {kind} {reference} reuses a retained timeline identity"
            ));
        }
    }
    Ok(())
}

fn message_refs_are_preserved(old: &RetainedConversation, new: &RetainedConversation) -> bool {
    new.messages.len() >= old.messages.len()
        && old
            .messages
            .iter()
            .zip(&new.messages)
            .all(|(old, new)| old.message_ref == new.message_ref)
}

#[cfg(test)]
pub(crate) fn validate_timeline_transition(
    old: &ConversationRuntimeState,
    new: &ConversationRuntimeState,
) -> CwrResult<()> {
    validate_retained_timelines(new)?;
    for (conversation_ref, old_timeline) in old.timeline_store.iter() {
        let new_timeline = new
            .timeline_store
            .timeline(conversation_ref)
            .ok_or_else(|| invalid_error("retained timelines are append-only"))?;
        validate_append_only(
            old_timeline,
            new_timeline,
            &old.streaming_messages
                .keys()
                .filter(|reference| !new.streaming_messages.contains_key(*reference))
                .map(String::as_str)
                .collect(),
        )?;
    }
    for old_conversation in &old.conversations {
        if let Some(old_root) = &old_conversation.timeline_connection_ref {
            let new_root = new
                .conversations
                .iter()
                .find(|conversation| {
                    conversation.conversation_ref == old_conversation.conversation_ref
                })
                .and_then(|conversation| conversation.timeline_connection_ref.as_ref());
            if new_root != Some(old_root) {
                return invalid("a retained conversation timeline root cannot change");
            }
        }
    }
    Ok(())
}

fn validate_timeline_content(
    state: &ConversationRuntimeState,
    conversation: &RetainedConversation,
    timeline: &ConversationTimeline,
    item: &ConversationTimelineItem,
) -> CwrResult<()> {
    match &item.content {
        ConversationTimelineContent::Message { message_ref } => {
            if !conversation
                .messages
                .iter()
                .any(|message| &message.message_ref == message_ref)
                && !state
                    .streaming_messages
                    .get(message_ref)
                    .is_some_and(|message| {
                        message.conversation_ref == conversation.conversation_ref
                    })
            {
                return invalid(format!(
                    "timeline item {} points outside its conversation messages",
                    item.item_ref
                ));
            }
        }
        ConversationTimelineContent::Invocation { invocation_ref } => {
            let invocation = state
                .invocation_ledger
                .invocation(invocation_ref)
                .ok_or_else(|| {
                    invalid_error(format!(
                        "timeline item {} points to a missing invocation",
                        item.item_ref
                    ))
                })?;
            if invocation.conversation_ref.as_deref()
                != Some(conversation.conversation_ref.as_str())
            {
                return invalid(format!(
                    "timeline item {} points to an invocation in another conversation",
                    item.item_ref
                ));
            }
            let disposition = invocation
                .timeline_publications
                .get(&item.publication_ref)
                .ok_or_else(|| {
                    invalid_error(format!(
                        "timeline item {} has no retained invocation publication decision",
                        item.item_ref
                    ))
                })?;
            if *disposition != timeline.disposition_for(item) {
                return invalid(format!(
                    "timeline item {} disagrees with its invocation publication decision",
                    item.item_ref
                ));
            }
        }
    }
    Ok(())
}

fn validate_append_only(
    old: &ConversationTimeline,
    new: &ConversationTimeline,
    withdrawable: &BTreeSet<&str>,
) -> CwrResult<()> {
    if old.root_connection_ref != new.root_connection_ref
        || new.connections.len() < old.connections.len()
    {
        return invalid("retained timelines are append-only");
    }
    for (old_connection, new_connection) in old.connections.iter().zip(&new.connections) {
        let retained = old_connection
            .items
            .iter()
            .filter(|item| {
                !withdrawable.contains(item.content.reference())
                    || new_connection.items.contains(item)
            })
            .cloned()
            .collect::<Vec<_>>();
        if old_connection.connection_ref != new_connection.connection_ref
            || old_connection.owner != new_connection.owner
            || !new_connection.items.starts_with(&retained)
        {
            return invalid("retained timeline connections and items are append-only");
        }
        let removals = u64::try_from(old_connection.items.len() - retained.len())?;
        let additions = u64::try_from(new_connection.items.len() - retained.len())?;
        for (index, item) in new_connection.items.iter().skip(retained.len()).enumerate() {
            if item.sibling_sequence
                != old_connection
                    .next_sequence
                    .checked_add(u64::try_from(index)?)
                    .ok_or("timeline sequence overflow")?
            {
                return invalid("new timeline items must not reuse withdrawn positions");
            }
        }

        if new_connection.revision
            != old_connection
                .revision
                .checked_add(additions + removals)
                .ok_or_else(|| invalid_error("timeline revision overflow"))?
            || new_connection.next_sequence
                != old_connection
                    .next_sequence
                    .checked_add(additions)
                    .ok_or_else(|| invalid_error("timeline sequence overflow"))?
        {
            return invalid("retained timeline counters must advance once per append");
        }
    }
    Ok(())
}
