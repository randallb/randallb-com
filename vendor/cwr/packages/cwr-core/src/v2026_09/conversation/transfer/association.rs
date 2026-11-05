use super::*;

pub(super) fn append_public_progress_origins(
    store: &ConversationAdmissionStore,
    conversation: &mut RetainedConversation,
) {
    let mut copied = conversation
        .activity
        .iter()
        .map(|activity| activity.activity_ref.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let candidates = conversation
        .messages
        .iter()
        .filter_map(|message| {
            let progress = message.progress.as_ref()?;
            (!copied.contains(&progress.source_activity_ref)).then(|| {
                (
                    progress.invocation_ref.clone(),
                    progress.source_activity_ref.clone(),
                    progress.status,
                    message.reply_to_message_ref.clone(),
                    message.author_actor_ref.clone(),
                )
            })
        })
        .collect::<Vec<_>>();
    let mut added = false;
    for (invocation_ref, activity_ref, retained_status, source_message_ref, actor_ref) in candidates
    {
        let Some(source_message_ref) = source_message_ref else {
            continue;
        };
        let Some(parent_invocation_ref) = store
            .invocation_record(&invocation_ref)
            .and_then(|record| record.parent_invocation_ref.clone())
        else {
            continue;
        };
        let Some(source) = store
            .execution_conversation(&invocation_ref)
            .and_then(|execution| {
                execution
                    .activity
                    .iter()
                    .find(|activity| activity.activity_ref == activity_ref)
            })
            .cloned()
        else {
            continue;
        };
        let (status, text, text_kind) = match &source.event {
            ConversationActivityEvent::Progress {
                status, summary, ..
            } => (
                *status,
                Some(summary.clone()),
                Some(WorkUpdateTextKind::Progress),
            ),
            ConversationActivityEvent::CommentaryProgress { text, .. } => (
                ActionResponseStatus::PROGRESS,
                Some(text.clone()),
                Some(WorkUpdateTextKind::Commentary),
            ),
            _ => continue,
        };
        if retained_status.is_some_and(|retained| retained != status) {
            continue;
        }
        conversation.activity.push(ConversationActivity {
            activity_ref: activity_ref.clone(),
            sequence: source.sequence,
            occurred_at_unix_ms: source.occurred_at_unix_ms,
            event: ConversationActivityEvent::WorkUpdate {
                invocation_ref,
                source_message_ref,
                parent_invocation_ref: Some(parent_invocation_ref),
                wait: None,
                actor_ref,
                status,
                text,
                text_kind,
                activity: None,
            },
        });
        copied.insert(activity_ref);
        added = true;
    }
    if added {
        crate::v2026_09::host::sort_work_activities(&mut conversation.activity);
    }
}

pub(super) fn append_inferred_message_work_associations(
    conversation: &mut RetainedConversation,
    occurred_at_unix_ms: u64,
) {
    let work_matches = WorkMatchIndex::new(conversation);
    let already_associated = conversation
        .activity
        .iter()
        .filter_map(|activity| match &activity.event {
            ConversationActivityEvent::MessageWorkAssociation { message_ref, .. } => {
                Some(message_ref.as_str())
            }
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    let candidates = conversation
        .messages
        .iter()
        .filter(|message| !already_associated.contains(message.message_ref.as_str()))
        .filter_map(|message| {
            let activity = work_matches.unique_match(message, conversation)?;
            Some((message.message_ref.clone(), activity.activity_ref.clone()))
        })
        .collect::<Vec<_>>();
    let mut target_counts = BTreeMap::<String, usize>::new();
    for (_, activity_ref) in &candidates {
        *target_counts.entry(activity_ref.clone()).or_default() += 1;
    }
    for (message_ref, activity_ref) in candidates {
        if target_counts.get(&activity_ref) != Some(&1) {
            continue;
        }
        conversation.activity.push(ConversationActivity {
            activity_ref: format!(
                "{}/transfer-associations/v1/{}",
                conversation.conversation_ref,
                transfer_hash(&message_ref),
            ),
            sequence: conversation.activity.len() as u64 + 1,
            occurred_at_unix_ms,
            event: ConversationActivityEvent::MessageWorkAssociation {
                message_ref,
                activity_ref,
            },
        });
    }
}

#[derive(Default)]
pub(super) struct WorkMatchIndex {
    terminal_by_reply: BTreeMap<(String, String), (usize, usize)>,
    activity_by_ref: BTreeMap<String, usize>,
}

impl WorkMatchIndex {
    pub(super) fn new(conversation: &RetainedConversation) -> Self {
        let mut index = Self::default();
        for (activity_index, activity) in conversation.activity.iter().enumerate() {
            let ConversationActivityEvent::WorkUpdate {
                source_message_ref,
                actor_ref,
                status,
                ..
            } = &activity.event
            else {
                continue;
            };
            index
                .activity_by_ref
                .insert(activity.activity_ref.clone(), activity_index);
            if status.is_terminal() {
                index
                    .terminal_by_reply
                    .entry((source_message_ref.clone(), actor_ref.clone()))
                    .and_modify(|(count, last)| {
                        *count += 1;
                        *last = activity_index;
                    })
                    .or_insert((1, activity_index));
            }
        }
        index
    }

    pub(super) fn unique_match<'a>(
        &self,
        message: &ConversationMessage,
        conversation: &'a RetainedConversation,
    ) -> Option<&'a ConversationActivity> {
        if let Some(progress) = &message.progress {
            let index = *self.activity_by_ref.get(&progress.source_activity_ref)?;
            let activity = &conversation.activity[index];
            return work_update_matches_message(activity, message).then_some(activity);
        }
        let source_message_ref = message.reply_to_message_ref.as_ref()?;
        let (count, index) = self
            .terminal_by_reply
            .get(&(source_message_ref.clone(), message.author_actor_ref.clone()))?;
        (*count == 1).then_some(&conversation.activity[*index])
    }

    pub(super) fn has_match(
        &self,
        message: &ConversationMessage,
        conversation: &RetainedConversation,
    ) -> bool {
        if let Some(progress) = &message.progress {
            return self
                .activity_by_ref
                .get(&progress.source_activity_ref)
                .is_some_and(|index| {
                    work_update_matches_message(&conversation.activity[*index], message)
                });
        }
        let Some(source_message_ref) = &message.reply_to_message_ref else {
            return false;
        };
        self.terminal_by_reply
            .contains_key(&(source_message_ref.clone(), message.author_actor_ref.clone()))
    }
}

pub(super) fn work_update_matches_message(
    activity: &ConversationActivity,
    message: &ConversationMessage,
) -> bool {
    let ConversationActivityEvent::WorkUpdate {
        source_message_ref,
        actor_ref,
        status,
        ..
    } = &activity.event
    else {
        return false;
    };
    if message.reply_to_message_ref.as_deref() != Some(source_message_ref)
        || message.author_actor_ref != *actor_ref
    {
        return false;
    }
    message.progress.as_ref().map_or_else(
        || status.is_terminal(),
        |progress| {
            progress.source_activity_ref == activity.activity_ref
                && progress.status.is_none_or(|retained| retained == *status)
        },
    )
}

/// Portable work sources name the public message each update actually answers.
pub(in crate::v2026_09::conversation) fn normalize_associated_work_sources(
    conversation: &mut RetainedConversation,
) {
    // Canonical nested replies can address the root message while their work
    // was admitted from a child call. The portable conversation associates each
    // public update with the message it actually answers.
    let associations = conversation
        .activity
        .iter()
        .filter_map(|item| {
            if let ConversationActivityEvent::MessageWorkAssociation {
                message_ref,
                activity_ref,
            } = &item.event
            {
                let message = conversation
                    .messages
                    .iter()
                    .find(|m| &m.message_ref == message_ref)?;
                Some((activity_ref.clone(), message.reply_to_message_ref.clone()?))
            } else {
                None
            }
        })
        .collect::<BTreeMap<_, _>>();
    for item in &mut conversation.activity {
        if let ConversationActivityEvent::WorkUpdate {
            source_message_ref, ..
        } = &mut item.event
        {
            if let Some(reply) = associations.get(&item.activity_ref) {
                *source_message_ref = reply.clone();
            }
        }
    }
}
