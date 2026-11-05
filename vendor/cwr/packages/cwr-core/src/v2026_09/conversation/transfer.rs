use super::*;
use crate::v2026_09::invocation::ActionInvocationState;

mod association;
mod timeline;
mod validation;
pub(super) use association::normalize_associated_work_sources;
use association::{
    append_inferred_message_work_associations, append_public_progress_origins,
    work_update_matches_message, WorkMatchIndex,
};
pub use timeline::ConversationTimelineExport;
use validation::validate_export;

pub(super) const EXPORT_VERSION_V1: &str = "cwr.conversation-export/1";
pub(super) const EXPORT_VERSION_V3: &str = "cwr.conversation-export/3";
pub(super) const EXPORT_VERSION: &str = "cwr.conversation-export/2";

/// Shared history only. Execution state and provider sessions are never imported.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationExport {
    pub version: String,
    pub exported_at_unix_ms: u64,
    #[serde(serialize_with = "serialize_export_conversation")]
    pub conversation: RetainedConversation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<ConversationTimelineExport>,
}

fn serialize_export_conversation<S>(
    conversation: &RetainedConversation,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let mut value = serde_json::to_value(conversation).map_err(serde::ser::Error::custom)?;
    if let Value::Object(fields) = &mut value {
        fields.remove("archived");
        fields.remove("sidebar_eligible");
        fields.remove("owner_context_ref");
    }
    serde::Serialize::serialize(&value, serializer)
}

impl ConversationAdmissionStore {
    pub(super) fn transferred_conversation(
        &self,
        caller: &str,
        operation_id: &str,
    ) -> CwrResult<Option<RetainedConversation>> {
        let reference = import_conversation_ref(&self.state.runtime_root_ref, operation_id);
        let Some(conversation) = self.full_conversation(&reference)? else {
            return Ok(None);
        };
        if !conversation
            .participant_actor_refs
            .iter()
            .any(|actor| actor == caller)
        {
            return invalid("Conversation transfer retry requires participation");
        }
        Ok(Some(conversation))
    }

    pub(crate) fn imported_conversation(
        &self,
        caller: &str,
        import_id: &str,
    ) -> CwrResult<Option<RetainedConversation>> {
        self.transferred_conversation(caller, import_id)
    }

    pub fn export_conversation(
        &mut self,
        reference: &str,
        caller: &str,
    ) -> CwrResult<ConversationExport> {
        self.hydrate_all_cold_conversations()?;
        let conversation = self
            .conversation(reference)
            .ok_or_else(|| invalid_error("Export conversation was not found"))?;
        if !conversation
            .participant_actor_refs
            .iter()
            .any(|actor| actor == caller)
        {
            return invalid("Export requires conversation participation");
        }
        if conversation.parent_conversation_ref.is_some() {
            return invalid("Only shared conversations can be exported");
        }
        if conversation.timeline_connection_ref.is_some() {
            return self.export_timeline_conversation(conversation, caller);
        }
        if conversation
            .messages
            .iter()
            .any(|message| !message.file_refs.is_empty())
        {
            return invalid("Conversations with file attachments cannot be exported until file custody can be transferred");
        }
        let exported_at_unix_ms = current_unix_ms();
        let mut conversation =
            crate::v2026_09::host::project_conversation_with_authoritative_work(conversation, self);
        conversation.messages = conversation
            .messages
            .iter()
            .map(ConversationMessage::clone_for_external_delivery)
            .collect();
        // Archive state is shared presentation state, not transferable history.
        // Keeping it false also preserves the v1 envelope for older readers.
        conversation.archived = false;
        conversation
            .activity
            .retain(|item| public_activity(&item.event));
        self.append_message_work_associations(&mut conversation, "export", exported_at_unix_ms);
        Ok(ConversationExport {
            version: EXPORT_VERSION.into(),
            exported_at_unix_ms,
            conversation,
            timeline: None,
        })
    }

    pub fn import_conversation(
        &mut self,
        graph: &ResolvedCwrGraph,
        caller: &str,
        mut export: ConversationExport,
        import_id: Option<&str>,
    ) -> CwrResult<RetainedConversation> {
        export.conversation.owner_context_ref = None;
        let reference = import_id.map_or_else(
            || new_import_conversation_ref(&self.state.runtime_root_ref),
            |id| import_conversation_ref(&self.state.runtime_root_ref, id),
        );
        self.import_conversation_to(graph, caller, export, import_id, reference)
    }

    pub(super) fn import_conversation_to(
        &mut self,
        graph: &ResolvedCwrGraph,
        caller: &str,
        export: ConversationExport,
        import_id: Option<&str>,
        reference: String,
    ) -> CwrResult<RetainedConversation> {
        let canonical = export
            .timeline
            .as_ref()
            .map(|timeline| timeline.plan(&export))
            .transpose()?;
        self.import_history_to(graph, caller, export, import_id, reference, canonical)
    }

    pub(super) fn import_history_to(
        &mut self,
        graph: &ResolvedCwrGraph,
        caller: &str,
        export: ConversationExport,
        import_id: Option<&str>,
        reference: String,
        canonical: Option<super::fork::canonical::ForkTimeline>,
    ) -> CwrResult<RetainedConversation> {
        self.hydrate_all_cold_conversations()?;
        if !graph.actors.iter().any(|actor| actor.actor_ref == caller) {
            return invalid("Import requires a coworker in this workspace");
        }
        validate_export(&export)?;
        // An action retry must return the same fork, even if terminal delivery failed.
        if let Some(id) = import_id {
            if let Some(existing) = self.imported_conversation(caller, id)? {
                return Ok(existing);
            }
        }
        if canonical.is_none()
            && self
                .timeline_rollout
                .is_some_and(|rollout| rollout != ConversationTimelineRollout::Disabled)
        {
            return invalid(
                "Conversation transfer is unavailable while canonical timeline rollout is enabled",
            );
        }
        let infer_v1_associations = export.version == EXPORT_VERSION_V1;
        let mut conversation = export.conversation;
        if infer_v1_associations {
            append_inferred_message_work_associations(
                &mut conversation,
                export.exported_at_unix_ms,
            );
        }
        conversation.conversation_ref = reference.clone();
        conversation.parent_conversation_ref = None;
        conversation.archived = false;
        // Import creates an explicitly requested destination root. Presentation
        // state from the source is not part of the v1 transfer contract, so the
        // new root belongs in the destination sidebar even when the exported
        // history came from an action.
        conversation.sidebar_eligible = Some(true);
        // Historical authorship is data; it does not grant participation in the new chat.
        conversation.participant_actor_refs = vec![caller.into()];
        let remap = |value: &str| format!("{reference}/history/{}", transfer_hash(value));
        for message in &mut conversation.messages {
            message.message_ref = remap(&message.message_ref);
            message.reply_to_message_ref = message.reply_to_message_ref.as_deref().map(remap);
            if let Some(progress) = &mut message.progress {
                progress.invocation_ref = remap(&progress.invocation_ref);
                progress.source_activity_ref = remap(&progress.source_activity_ref);
            }
        }
        for (index, item) in conversation.activity.iter_mut().enumerate() {
            item.activity_ref = remap(&item.activity_ref);
            item.sequence = index as u64 + 1;
            match &mut item.event {
                ConversationActivityEvent::MessageRecorded { message_ref } => {
                    *message_ref = remap(message_ref)
                }
                ConversationActivityEvent::MessageWorkAssociation {
                    message_ref,
                    activity_ref,
                } => {
                    *message_ref = remap(message_ref);
                    *activity_ref = remap(activity_ref);
                }
                ConversationActivityEvent::WorkPhase { invocation_ref, .. } => {
                    *invocation_ref = remap(invocation_ref)
                }
                ConversationActivityEvent::WorkLifecycle {
                    invocation_ref,
                    source_message_ref,
                    parent_invocation_ref,
                    awaited_by_invocation_ref,
                    state,
                    finished_at_unix_ms,
                    ..
                } => {
                    *invocation_ref = remap(invocation_ref);
                    *source_message_ref = remap(source_message_ref);
                    *parent_invocation_ref = parent_invocation_ref.as_deref().map(remap);
                    *awaited_by_invocation_ref = None;
                    if !state.is_terminal() {
                        *state = ActionInvocationState::Stopped;
                        *finished_at_unix_ms = Some(export.exported_at_unix_ms);
                    }
                }
                ConversationActivityEvent::WorkUpdate {
                    invocation_ref,
                    source_message_ref,
                    parent_invocation_ref,
                    wait,
                    activity,
                    ..
                } => {
                    *invocation_ref = remap(invocation_ref);
                    *source_message_ref = remap(source_message_ref);
                    *parent_invocation_ref = parent_invocation_ref.as_deref().map(remap);
                    if let Some(wait) = wait {
                        wait.waiter_invocation_ref = remap(&wait.waiter_invocation_ref);
                        wait.awaited_invocation_ref = remap(&wait.awaited_invocation_ref);
                        wait.call_id = remap(&wait.call_id);
                    }
                    if let Some(activity) = activity {
                        activity.activity_ref = remap(&activity.activity_ref);
                        activity.child_invocation_ref =
                            activity.child_invocation_ref.as_deref().map(remap);
                        if matches!(
                            activity.state.as_str(),
                            "running" | "pending" | "working" | "waiting"
                        ) {
                            activity.state = "stopped".into();
                        }
                    }
                }
                ConversationActivityEvent::ConversationForkedFrom { .. } => {}
                _ => unreachable!("validated public activity"),
            }
        }
        let mut work = BTreeMap::new();
        let mut terminal = std::collections::BTreeSet::new();
        for item in &conversation.activity {
            if let ConversationActivityEvent::WorkUpdate {
                invocation_ref,
                status,
                ..
            } = &item.event
            {
                work.insert(invocation_ref.clone(), item.clone());
                if status.is_terminal() {
                    terminal.insert(invocation_ref.clone());
                }
            }
        }
        for (reference, mut item) in work {
            if terminal.contains(&reference) {
                continue;
            }
            item.activity_ref = format!("{reference}/import-stopped");
            item.sequence = conversation.activity.len() as u64 + 1;
            item.occurred_at_unix_ms = export.exported_at_unix_ms;
            if let ConversationActivityEvent::WorkUpdate {
                status,
                text,
                text_kind,
                activity,
                ..
            } = &mut item.event
            {
                *status = ActionResponseStatus::STOPPED;
                *text = None;
                *text_kind = None;
                *activity = None;
            }
            conversation.activity.push(item);
        }
        // Historical actors may be absent from the destination graph, but their refs
        // and every conversation-internal relationship must still be valid.
        crate::v2026_09::host::validate_root_conversation(&conversation)?;
        self.transact(|state| {
            state.conversations.push(conversation.clone());
            if let Some(plan) = canonical {
                plan.retain(state, &conversation)?;
            }
            Ok(())
        })?;
        self.committed_conversation(&reference)
    }

    pub(super) fn append_message_work_associations(
        &self,
        conversation: &mut RetainedConversation,
        namespace: &str,
        occurred_at_unix_ms: u64,
    ) {
        append_public_progress_origins(self, conversation);
        let copied_activity_refs = conversation
            .activity
            .iter()
            .map(|item| item.activity_ref.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let already_associated = conversation
            .activity
            .iter()
            .filter_map(|item| match &item.event {
                ConversationActivityEvent::MessageWorkAssociation { message_ref, .. } => {
                    Some(message_ref.clone())
                }
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        let selected_activity_refs = self.selected_message_activity_refs();
        let associations = conversation
            .messages
            .iter()
            .filter(|message| !already_associated.contains(&message.message_ref))
            .filter_map(|message| {
                selected_activity_refs
                    .get(&message.message_ref)
                    .cloned()
                    .filter(|activity_ref| copied_activity_refs.contains(activity_ref))
                    .map(|activity_ref| (message.message_ref.clone(), activity_ref))
            })
            .collect::<Vec<_>>();
        for (message_ref, activity_ref) in associations {
            conversation.activity.push(ConversationActivity {
                activity_ref: format!(
                    "{}/transfer-associations/{namespace}/{}",
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
        // Upgraded legacy state can lack the private invocation reply pointer
        // used above. Complete an otherwise exact association from the public
        // retained work so every v2 envelope validates when re-imported.
        append_inferred_message_work_associations(conversation, occurred_at_unix_ms);
    }

    pub(super) fn selected_message_activity_refs(&self) -> BTreeMap<String, String> {
        let mut selected = BTreeMap::new();
        let mut conversations_by_ref = BTreeMap::new();
        for conversation in self.conversations() {
            conversations_by_ref.insert(conversation.conversation_ref.as_str(), conversation);
            for item in &conversation.activity {
                if let ConversationActivityEvent::MessageWorkAssociation {
                    message_ref,
                    activity_ref,
                } = &item.event
                {
                    selected.insert(message_ref.clone(), activity_ref.clone());
                }
            }
            for message in &conversation.messages {
                if let Some(progress) = &message.progress {
                    selected
                        .entry(message.message_ref.clone())
                        .or_insert_with(|| progress.source_activity_ref.clone());
                }
            }
        }

        let mut replies = BTreeMap::<String, String>::new();
        for record in self.admissions() {
            if let (Some(message_ref), Some(invocation_ref)) =
                (&record.reply_message_ref, &record.invocation_ref)
            {
                replies.insert(message_ref.clone(), invocation_ref.clone());
            }
        }
        for record in self.invocation_records() {
            if let Some(message_ref) = &record.reply_message_ref {
                replies
                    .entry(message_ref.clone())
                    .or_insert_with(|| record.invocation_ref.clone());
            }
        }
        for (message_ref, invocation_ref) in replies {
            if selected.contains_key(&message_ref) {
                continue;
            }
            let Some(activity_ref) = self
                .execution_conversation_ref(&invocation_ref)
                .and_then(|conversation_ref| conversations_by_ref.get(conversation_ref).copied())
                .and_then(|conversation| {
                    conversation
                        .activity
                        .iter()
                        .rev()
                        .find_map(|item| match item.event {
                            ConversationActivityEvent::TurnCompleted { .. }
                            | ConversationActivityEvent::TurnFailed { .. }
                            | ConversationActivityEvent::TurnStopped { .. } => {
                                Some(item.activity_ref.clone())
                            }
                            _ => None,
                        })
                })
            else {
                continue;
            };
            selected.insert(message_ref, activity_ref);
        }
        selected
    }
}

pub(super) fn transfer_hash(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn import_conversation_ref(runtime_root_ref: &str, import_id: &str) -> String {
    format!(
        "{runtime_root_ref}/conversations/import-{}",
        transfer_hash(import_id)
    )
}

fn new_import_conversation_ref(runtime_root_ref: &str) -> String {
    let reference = new_ref(runtime_root_ref, "conversations");
    let identity = reference.rsplit('/').next().unwrap_or("conversation");
    format!("{runtime_root_ref}/conversations/import-{identity}")
}

pub(crate) fn is_imported_conversation_ref(reference: &str) -> bool {
    reference
        .rsplit('/')
        .next()
        .is_some_and(|identity| identity.starts_with("import-") || identity.starts_with("fork-"))
}

pub(super) fn public_activity(event: &ConversationActivityEvent) -> bool {
    matches!(
        event,
        ConversationActivityEvent::MessageRecorded { .. }
            | ConversationActivityEvent::MessageWorkAssociation { .. }
            | ConversationActivityEvent::ConversationForkedFrom { .. }
            | ConversationActivityEvent::WorkUpdate { .. }
            | ConversationActivityEvent::WorkLifecycle { .. }
            | ConversationActivityEvent::WorkPhase { .. }
    )
}
