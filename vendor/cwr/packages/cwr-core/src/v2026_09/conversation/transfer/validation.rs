use super::*;

pub(super) fn validate_export(export: &ConversationExport) -> CwrResult<()> {
    if export.version != EXPORT_VERSION
        && export.version != EXPORT_VERSION_V1
        && export.version != EXPORT_VERSION_V3
    {
        return invalid("Unsupported conversation export version");
    }
    if export.version == EXPORT_VERSION_V1
        && export.conversation.activity.iter().any(|item| {
            matches!(
                item.event,
                ConversationActivityEvent::MessageWorkAssociation { .. }
                    | ConversationActivityEvent::ConversationForkedFrom { .. }
            )
        })
    {
        return invalid("Conversation export v1 cannot contain fork metadata");
    }
    if (export.version == EXPORT_VERSION_V3) != export.timeline.is_some() {
        return invalid("Conversation export version does not match its timeline");
    }
    if let Some(timeline) = &export.timeline {
        timeline.validate(export)?;
    }
    let conversation = &export.conversation;
    if conversation.parent_conversation_ref.is_some() {
        return invalid("Cannot import a private execution conversation");
    }
    if conversation.timeline_connection_ref.is_some() {
        return invalid(
            "Conversation export cannot contain a retained timeline until timeline transfer is supported",
        );
    }
    if let Some(title) = &conversation.title {
        if normalize_conversation_title(title)? != *title {
            return invalid("Conversation export title must already be normalized");
        }
    }
    let mut messages = std::collections::BTreeSet::new();
    for message in &conversation.messages {
        if !message.file_refs.is_empty() {
            return invalid(
                "Conversation import cannot contain file references without transferred custody",
            );
        }
        if !message.visualizations.is_empty() {
            return invalid("Conversation export cannot contain visualization artifacts");
        }
        if message.message_ref.is_empty() || !messages.insert(&message.message_ref) {
            return invalid("Export has duplicate or empty message identities");
        }
    }
    if conversation.messages.iter().any(|message| {
        message
            .reply_to_message_ref
            .as_ref()
            .is_some_and(|reference| !messages.contains(reference))
    }) {
        return invalid("Export has a reply to a missing message");
    }
    let messages_by_ref = conversation
        .messages
        .iter()
        .map(|message| (message.message_ref.as_str(), message))
        .collect::<BTreeMap<_, _>>();
    let mut activities = std::collections::BTreeSet::new();
    let mut recorded_messages = std::collections::BTreeSet::new();
    let mut associated_messages = std::collections::BTreeSet::new();
    let mut associated_work = std::collections::BTreeSet::new();
    for item in &conversation.activity {
        if !public_activity(&item.event)
            || item.activity_ref.is_empty()
            || !activities.insert(&item.activity_ref)
        {
            return invalid("Export contains private activity or duplicate activity identities");
        }
        let source = match &item.event {
            ConversationActivityEvent::MessageRecorded { message_ref } => {
                if !recorded_messages.insert(message_ref) {
                    return invalid("Export contains duplicate message boundary activity");
                }
                Some(message_ref)
            }
            ConversationActivityEvent::MessageWorkAssociation { message_ref, .. } => {
                Some(message_ref)
            }
            _ => None,
        };
        if source.is_some_and(|reference| !messages.contains(reference)) {
            return invalid("Export work references a missing message");
        }
    }
    if recorded_messages != messages {
        return invalid("Export is missing message boundary activity");
    }
    let activity_by_ref = conversation
        .activity
        .iter()
        .map(|activity| (activity.activity_ref.as_str(), activity))
        .collect::<BTreeMap<_, _>>();
    let work_matches = WorkMatchIndex::new(conversation);
    for item in &conversation.activity {
        let ConversationActivityEvent::MessageWorkAssociation {
            message_ref,
            activity_ref,
        } = &item.event
        else {
            continue;
        };
        if !associated_messages.insert(message_ref) || !associated_work.insert(activity_ref) {
            return invalid("Export contains ambiguous message-work associations");
        }
        let Some(message) = messages_by_ref.get(message_ref.as_str()) else {
            return invalid("Export association refers to a missing message");
        };
        let Some(work) = activity_by_ref.get(activity_ref.as_str()) else {
            return invalid("Export associates a message with missing work");
        };
        if !work_update_matches_message(work, message) {
            return invalid("Export associates a message with unrelated work");
        }
    }
    if export.version != EXPORT_VERSION_V1
        && conversation.messages.iter().any(|message| {
            !associated_messages.contains(&message.message_ref)
                && work_matches.has_match(message, conversation)
        })
    {
        return invalid("Conversation export v2 is missing a message-work association");
    }
    Ok(())
}
