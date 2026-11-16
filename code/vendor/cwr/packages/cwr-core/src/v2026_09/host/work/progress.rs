fn latest_keyed_progress(execution: &RetainedConversation) -> BTreeMap<&str, &str> {
    execution
        .activity
        .iter()
        .filter_map(|item| match &item.event {
            ConversationActivityEvent::Progress {
                progress: Some(progress),
                ..
            } => Some((progress.key.as_str(), item.activity_ref.as_str())),
            _ => None,
        })
        .collect()
}

fn work_update_text_kind(
    event: &ConversationActivityEvent,
) -> Option<crate::v2026_09::conversation::WorkUpdateTextKind> {
    use crate::v2026_09::conversation::WorkUpdateTextKind;

    match event {
        ConversationActivityEvent::Progress { .. } => Some(WorkUpdateTextKind::Progress),
        ConversationActivityEvent::CommentaryProgress { .. } => {
            Some(WorkUpdateTextKind::Commentary)
        }
        _ => None,
    }
}
