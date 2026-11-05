use super::*;

/// Internal activation and migration policies for canonical timelines.
///
/// Direct `ConversationAdmissionStore` users remain in manual mode until a
/// local runtime configures one of these stages. Canonical reads are selected
/// by retained state, not by this switch.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ConversationTimelineRollout {
    /// Do not activate new conversations. Continue maintaining existing
    /// canonical timelines so turning off the startup flag preserves history.
    #[default]
    Disabled,
    /// Create timelines for new shared conversations and maintain every
    /// canonical conversation through the normal atomic mutation path.
    NewConversations,
    /// Also migrate compatible quiescent legacy conversations before serving.
    MigrateLegacy,
}

#[derive(Default)]
struct ChangedConversation {
    is_new: bool,
    new_message_refs: Vec<String>,
}

impl ConversationAdmissionStore {
    #[cfg(test)]
    pub(crate) fn configure_timeline_rollout(
        &mut self,
        rollout: ConversationTimelineRollout,
    ) -> CwrResult<()> {
        let mut backend = self
            .backend
            .lock()
            .map_err(|_| invalid_error("conversation backend lock poisoned"))?;
        backend.set_expected_revision(self.commit_revision);
        let metadata = backend
            .pages(cwpdb_store::conversation::Selection::Bootstrap)?
            .metadata;
        let retained: ConversationTimelineRollout = serde_json::from_value(
            metadata
                .get("conversation_timeline_rollout")
                .cloned()
                .unwrap_or_else(|| serde_json::json!("disabled")),
        )?;
        if rollout > retained {
            self.commit_revision = backend.commit_changes(cwpdb_store::conversation::Changes {
                timeline_rollout: Some(
                    serde_json::to_value(rollout)?
                        .as_str()
                        .ok_or("invalid timeline rollout stage")?
                        .into(),
                ),
                ..Default::default()
            })?;
        }
        self.timeline_rollout = Some(rollout);
        Ok(())
    }

    pub(crate) fn migrate_eligible_legacy_timelines(&mut self) -> CwrResult<()> {
        if self.timeline_rollout != Some(ConversationTimelineRollout::MigrateLegacy) {
            return Ok(());
        }
        let conversation_refs = self
            .state
            .conversations
            .iter()
            .filter(|conversation| {
                conversation.parent_conversation_ref.is_none()
                    && conversation.timeline_connection_ref.is_none()
            })
            .map(|conversation| conversation.conversation_ref.clone())
            .collect::<Vec<_>>();
        for conversation_ref in conversation_refs {
            self.migrate_legacy_conversation_timeline(&conversation_ref)?;
        }
        Ok(())
    }
}

pub(crate) fn stage_timeline_rollout_changes(
    state: &mut ConversationRuntimeState,
    rollout: ConversationTimelineRollout,
) -> CwrResult<()> {
    // Disabling activation must not strand existing canonical history. Keep its
    // normal writes coherent, while creating no new timeline roots.
    // Provisional text updates do not change retained messages, invocation
    // publication decisions or topology. Avoid scanning retained history per delta.
    if state.conversations.changes().next().is_none()
        && state.invocation_ledger.changes().next().is_none()
        && state.timeline_store.changes().next().is_none()
    {
        return Ok(());
    }
    validate_managed_parent_placements(state)?;

    let changes = changed_conversations(state);
    let message_owners = changed_message_owners(state)?;
    let new_invocations = new_invocation_refs(state);
    let mut migrated = BTreeSet::new();
    for (conversation_ref, change) in &changes {
        let Some(conversation) = state
            .conversations
            .iter()
            .find(|conversation| conversation.conversation_ref == *conversation_ref)
        else {
            continue;
        };
        if conversation.parent_conversation_ref.is_some() {
            continue;
        }
        if conversation.timeline_connection_ref.is_none() {
            if change.is_new && rollout != ConversationTimelineRollout::Disabled {
                create_managed_timeline(state, conversation_ref)?;
            } else if rollout == ConversationTimelineRollout::MigrateLegacy
                && matches!(
                    super::migration::stage_legacy_timeline_migration(state, conversation_ref)?,
                    ConversationTimelineMigrationOutcome::Migrated { .. }
                )
            {
                migrated.insert(conversation_ref.clone());
            }
        }
    }

    for (conversation_ref, change) in &changes {
        if migrated.contains(conversation_ref)
            || state.timeline_store.timeline(conversation_ref).is_none()
        {
            continue;
        }
        for message_ref in &change.new_message_refs {
            publish_managed_message(state, conversation_ref, message_ref, &message_owners)?;
        }
    }
    for invocation_ref in new_invocations {
        publish_managed_invocation(state, &invocation_ref)?;
    }
    Ok(())
}

fn validate_managed_parent_placements(state: &ConversationRuntimeState) -> CwrResult<()> {
    let mut invocation_refs = BTreeSet::new();
    for (_, timeline) in state.timeline_store.iter() {
        for item in timeline
            .connections
            .iter()
            .flat_map(|connection| &connection.items)
        {
            let ConversationTimelineContent::Invocation { invocation_ref } = &item.content else {
                continue;
            };
            // Copied work is immutable and cannot emit new managed children.
            // Its historical placements may repeat; only live execution needs
            // one unambiguous parent placement for subsequent publications.
            if state
                .invocation_ledger
                .invocation(invocation_ref)
                .is_some_and(|record| record.forked_from_invocation_ref.is_some())
            {
                continue;
            }
            if !invocation_refs.insert(invocation_ref) {
                return invalid(
                    "conversation timeline rollout cannot manage repeated invocation placements",
                );
            }
        }
    }
    Ok(())
}

fn changed_conversations(
    state: &ConversationRuntimeState,
) -> BTreeMap<String, ChangedConversation> {
    let mut changes = BTreeMap::new();
    for (_, old, conversation) in state.conversations.changes() {
        if conversation.parent_conversation_ref.is_some() {
            continue;
        }
        let old_message_count = old.map_or(0, |old| old.messages.len());
        changes.insert(
            conversation.conversation_ref.clone(),
            ChangedConversation {
                is_new: old.is_none(),
                new_message_refs: conversation
                    .messages
                    .iter()
                    .skip(old_message_count)
                    .map(|message| message.message_ref.clone())
                    .collect(),
            },
        );
    }
    for (_, _, invocation) in state.invocation_ledger.changes() {
        let Some(conversation_ref) = invocation.and_then(|record| record.conversation_ref.as_ref())
        else {
            continue;
        };
        changes.entry(conversation_ref.clone()).or_default();
    }
    changes
}

fn new_invocation_refs(state: &ConversationRuntimeState) -> Vec<String> {
    let mut invocations = state
        .invocation_ledger
        .changes()
        .filter_map(|(_, old, new)| {
            (old.is_none())
                .then_some(new?)
                .map(|record| (record.admission_sequence, record.invocation_ref.clone()))
        })
        .collect::<Vec<_>>();
    invocations.sort();
    invocations
        .into_iter()
        .map(|(_, invocation_ref)| invocation_ref)
        .collect()
}

fn changed_message_owners(
    state: &ConversationRuntimeState,
) -> CwrResult<BTreeMap<String, BTreeSet<String>>> {
    let mut owners = BTreeMap::<String, BTreeSet<String>>::new();
    for (_, old, conversation) in state.conversations.changes() {
        let old_message_count = old.map_or(0, |old| old.messages.len());
        for message in conversation.messages.iter().skip(old_message_count) {
            if let Some(progress) = &message.progress {
                add_message_owner(&mut owners, &message.message_ref, &progress.invocation_ref);
            }
        }
        add_changed_host_call_owners(old, conversation, &mut owners)?;
    }
    for (_, old, new) in state.invocation_ledger.changes() {
        let Some(invocation) = new else {
            continue;
        };
        if invocation.reply_message_ref.as_deref()
            != old.and_then(|record| record.reply_message_ref.as_deref())
        {
            if let Some(message_ref) = &invocation.reply_message_ref {
                add_message_owner(&mut owners, message_ref, &invocation.invocation_ref);
            }
        }
        if old.is_none() && invocation.forked_from_invocation_ref.is_none() {
            if let Some(parent_ref) = invocation.parent_invocation_ref.as_deref() {
                let message_ref =
                    retained_invocation_source_message_ref(state, &invocation.invocation_ref)?;
                add_message_owner(&mut owners, message_ref, parent_ref);
            }
        }
    }
    for (_, old, admission) in state.admissions.changes() {
        if admission.reply_message_ref.as_deref()
            == old.and_then(|record| record.reply_message_ref.as_deref())
        {
            continue;
        }
        if let (Some(message_ref), Some(invocation_ref)) = (
            admission.reply_message_ref.as_deref(),
            admission.invocation_ref.as_deref(),
        ) {
            add_message_owner(&mut owners, message_ref, invocation_ref);
        }
    }
    Ok(owners)
}

fn add_changed_host_call_owners(
    old: Option<&RetainedConversation>,
    conversation: &RetainedConversation,
    owners: &mut BTreeMap<String, BTreeSet<String>>,
) -> CwrResult<()> {
    let Some(shared_conversation_ref) = conversation.parent_conversation_ref.as_deref() else {
        return Ok(());
    };
    let old_activity_count = old.map_or(0, |old| old.activity.len());
    let call_message_refs = conversation
        .activity
        .iter()
        .skip(old_activity_count)
        .filter_map(|activity| match &activity.event {
            super::super::ConversationActivityEvent::HostActionStarted {
                call_message_ref, ..
            } => Some(call_message_ref),
            _ => None,
        })
        .collect::<Vec<_>>();
    if call_message_refs.is_empty() {
        return Ok(());
    }
    let invocation_refs = conversation
        .activity
        .iter()
        .filter_map(|activity| match &activity.event {
            super::super::ConversationActivityEvent::TurnStarted { invocation_ref, .. } => {
                Some(invocation_ref.as_str())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if invocation_refs.len() != 1 {
        return invalid(format!(
            "managed execution conversation for {shared_conversation_ref} has ambiguous ownership"
        ));
    }
    let invocation_ref = *invocation_refs
        .iter()
        .next()
        .expect("one execution-conversation owner was checked");
    for message_ref in call_message_refs {
        add_message_owner(owners, message_ref, invocation_ref);
    }
    Ok(())
}

fn add_message_owner(
    owners: &mut BTreeMap<String, BTreeSet<String>>,
    message_ref: &str,
    invocation_ref: &str,
) {
    owners
        .entry(message_ref.to_owned())
        .or_default()
        .insert(invocation_ref.to_owned());
}

fn create_managed_timeline(
    state: &mut ConversationRuntimeState,
    conversation_ref: &str,
) -> CwrResult<()> {
    let root_connection_ref =
        crate::cwpdb::derived_ref("managed_conversation_timeline_root", &[conversation_ref]);
    state
        .timeline_store
        .create(conversation_ref, &root_connection_ref)?;
    let conversation = state
        .conversations
        .find_mut(|conversation| conversation.conversation_ref == conversation_ref)
        .ok_or_else(|| invalid_error("managed timeline conversation disappeared"))?;
    conversation.timeline_connection_ref = Some(root_connection_ref);
    Ok(())
}

fn publish_managed_message(
    state: &mut ConversationRuntimeState,
    conversation_ref: &str,
    message_ref: &str,
    message_owners: &BTreeMap<String, BTreeSet<String>>,
) -> CwrResult<()> {
    if timeline_contains_content(state, conversation_ref, message_ref) {
        return Ok(());
    }
    let disposition = match changed_message_owner(message_owners, message_ref)? {
        Some(invocation_ref) => managed_child_disposition(state, conversation_ref, invocation_ref)?,
        None => ConversationTimelineDisposition::Root,
    };
    let publication_ref = managed_publication_ref(conversation_ref, "message", message_ref);
    state.timeline_store.publish(
        conversation_ref,
        publication_ref,
        ConversationTimelineContent::Message {
            message_ref: message_ref.to_owned(),
        },
        disposition,
    )?;
    Ok(())
}

fn publish_managed_invocation(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<()> {
    let record = state
        .invocation_ledger
        .invocation(invocation_ref)
        .ok_or_else(|| invalid_error("managed timeline invocation disappeared"))?;
    let Some(conversation_ref) = record.conversation_ref.clone() else {
        return Ok(());
    };
    if state.timeline_store.timeline(&conversation_ref).is_none()
        || !record.timeline_publications.is_empty()
    {
        return Ok(());
    }
    let disposition = match record.parent_invocation_ref.clone() {
        Some(parent_ref) => managed_child_disposition(state, &conversation_ref, &parent_ref)?,
        None => ConversationTimelineDisposition::Root,
    };
    let source_message_ref =
        retained_invocation_source_message_ref(state, invocation_ref)?.to_owned();
    let publication_ref = managed_publication_ref(&conversation_ref, "invocation", invocation_ref);
    state.invocation_ledger.retain_timeline_publication(
        invocation_ref,
        &source_message_ref,
        &publication_ref,
        disposition.clone(),
    )?;
    state.timeline_store.publish(
        &conversation_ref,
        publication_ref,
        ConversationTimelineContent::Invocation {
            invocation_ref: invocation_ref.to_owned(),
        },
        disposition,
    )?;
    Ok(())
}

pub(in crate::v2026_09::conversation) fn managed_publication_ref(
    conversation_ref: &str,
    kind: &str,
    content_ref: &str,
) -> String {
    crate::cwpdb::derived_ref(
        "managed_conversation_timeline_publication",
        &[conversation_ref, kind, content_ref],
    )
}

fn timeline_contains_content(
    state: &ConversationRuntimeState,
    conversation_ref: &str,
    content_ref: &str,
) -> bool {
    state
        .timeline_store
        .timeline(conversation_ref)
        .is_some_and(|timeline| {
            timeline
                .connections
                .iter()
                .flat_map(|connection| &connection.items)
                .any(|item| item.content.reference() == content_ref)
        })
}

fn managed_child_disposition(
    state: &ConversationRuntimeState,
    conversation_ref: &str,
    invocation_ref: &str,
) -> CwrResult<ConversationTimelineDisposition> {
    let timeline = state
        .timeline_store
        .timeline(conversation_ref)
        .ok_or_else(|| invalid_error("managed timeline is unavailable"))?;
    let mut items = timeline
        .connections
        .iter()
        .flat_map(|connection| &connection.items)
        .filter(|item| {
            matches!(
                &item.content,
                ConversationTimelineContent::Invocation {
                    invocation_ref: retained
                } if retained == invocation_ref
            )
        });
    if let Some(item_ref) = items.next().map(|item| item.item_ref.clone()) {
        if items.next().is_some() {
            return invalid("managed timeline parent invocation has ambiguous placements");
        }
        return Ok(ConversationTimelineDisposition::Child {
            parent_item_ref: item_ref,
        });
    }
    let invocation = state
        .invocation_ledger
        .invocation(invocation_ref)
        .filter(|record| record.conversation_ref.as_deref() == Some(conversation_ref))
        .ok_or_else(|| invalid_error("managed timeline parent invocation is unavailable"))?;
    if !invocation.timeline_publications.is_empty()
        && invocation
            .timeline_publications
            .values()
            .all(|disposition| *disposition == ConversationTimelineDisposition::Undisclosed)
    {
        return Ok(ConversationTimelineDisposition::Undisclosed);
    }
    invalid("managed timeline parent invocation is not placed")
}

fn changed_message_owner<'a>(
    owners: &'a BTreeMap<String, BTreeSet<String>>,
    message_ref: &str,
) -> CwrResult<Option<&'a str>> {
    let Some(owners) = owners.get(message_ref) else {
        return Ok(None);
    };
    match owners.len() {
        0 => Ok(None),
        1 => Ok(owners.iter().next().map(String::as_str)),
        _ => invalid("managed timeline message has conflicting invocation owners"),
    }
}
