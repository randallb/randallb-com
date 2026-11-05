//! Portable publication topology and settled public work, without execution inputs.
use super::*;
use crate::v2026_09::invocation::{
    ActionInvocationRecord, ActionResponse, ActionResponseDetail, InvocationOrigin,
};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationTimelineExport {
    pub timeline: ConversationTimeline,
    pub work: Vec<ExportedWork>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExportedWork {
    pub invocation_ref: String,
    pub requester_actor_ref: String,
    pub target_actor_ref: String,
    pub action_name: String,
    pub source_message_ref: Option<String>,
    pub reply_message_ref: Option<String>,
    pub parent_invocation_ref: Option<String>,
    pub admission_sequence: u64,
    pub admitted_at_unix_ms: u64,
    pub started_at_unix_ms: Option<u64>,
    pub finished_at_unix_ms: u64,
    pub status: ActionResponseStatus,
    pub summary: String,
    pub activities: Vec<ExportedActivity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExportedActivity {
    pub first_sequence: u64,
    pub sequence: u64,
    pub activity: WorkActivity,
}

impl ConversationAdmissionStore {
    pub(super) fn export_timeline_conversation(
        &self,
        source: &RetainedConversation,
        caller: &str,
    ) -> CwrResult<ConversationExport> {
        let timeline = self
            .retained_timeline(&source.conversation_ref)
            .ok_or_else(|| invalid_error("Export timeline is missing"))?
            .clone();
        let messages = timeline
            .connections
            .iter()
            .flat_map(|c| &c.items)
            .filter_map(|item| match &item.content {
                ConversationTimelineContent::Message { message_ref } => Some(message_ref.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let published_work = timeline
            .connections
            .iter()
            .flat_map(|c| &c.items)
            .filter_map(|item| match &item.content {
                ConversationTimelineContent::Invocation { invocation_ref } => {
                    Some(invocation_ref.clone())
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let (path, runtime) = self.read_store_identity();
        let inspector = ConversationPublicationSnapshot::open(
            &path,
            &runtime,
            caller,
            std::time::Instant::now() + std::time::Duration::from_secs(30),
            ConversationReadCancellation::pending(),
        )?;
        let details = published_work
            .iter()
            .map(|reference| {
                Ok((
                    reference.clone(),
                    inspector.can_export_inspector_details(reference)?,
                ))
            })
            .collect::<CwrResult<BTreeMap<_, _>>>()?;
        let time = current_unix_ms();
        let mut conversation =
            crate::v2026_09::host::project_conversation_with_authoritative_work(source, self);
        // A visible provider stream has a placement before it is finalized as
        // a conversation message. Export its current text as an immutable copy.
        for stream in self.state.streaming_messages.values().filter(|stream| {
            stream.conversation_ref == source.conversation_ref
                && messages.contains(&stream.message.message_ref)
        }) {
            conversation.messages.push(stream.message.clone());
        }
        conversation.timeline_connection_ref = None;
        conversation.archived = false;
        conversation
            .messages
            .retain(|m| messages.contains(&m.message_ref));
        if conversation
            .messages
            .iter()
            .any(|m| !m.file_refs.is_empty())
        {
            return invalid("Conversations with file attachments cannot be exported until file custody can be transferred");
        }
        for message in &mut conversation.messages {
            *message = message.clone_for_external_delivery();
            if message
                .reply_to_message_ref
                .as_ref()
                .is_some_and(|r| !messages.contains(r))
            {
                message.reply_to_message_ref = None;
            }
        }
        retain_message_boundaries(&mut conversation, time);
        // Inspector operations already retain their latest accepted event and
        // first execution position. Projected compatibility updates are sorted by
        // wall clock and must not choose the current operation snapshot.
        let mut activities = BTreeMap::<String, Vec<ExportedActivity>>::new();
        for operation in self.state.tool_operations.values() {
            if details.get(&operation.invocation_ref) != Some(&true) {
                continue;
            }
            let mut activity = operation.activity.clone();
            settle_activity(&mut activity);
            activity.child_invocation_ref = activity
                .child_invocation_ref
                .filter(|r| published_work.contains(r));
            activities
                .entry(operation.invocation_ref.clone())
                .or_default()
                .push(ExportedActivity {
                    first_sequence: operation.first_sequence,
                    sequence: operation.sequence,
                    activity,
                });
        }
        for items in activities.values_mut() {
            items.sort_by(|a, b| {
                (a.first_sequence, &a.activity.activity_ref)
                    .cmp(&(b.first_sequence, &b.activity.activity_ref))
            });
        }
        for item in &mut conversation.activity {
            if let ConversationActivityEvent::WorkUpdate {
                invocation_ref,
                activity,
                ..
            } = &mut item.event
            {
                if details.get(invocation_ref) != Some(&true) {
                    *activity = None;
                } else if let Some(activity) = activity {
                    settle_activity(activity);
                }
            }
        }
        conversation.activity.retain(|item| {
            public_activity(&item.event)
                && match &item.event {
                    ConversationActivityEvent::MessageRecorded { message_ref }
                    | ConversationActivityEvent::MessageWorkAssociation { message_ref, .. } => {
                        messages.contains(message_ref)
                    }
                    ConversationActivityEvent::WorkUpdate {
                        source_message_ref,
                        invocation_ref,
                        ..
                    }
                    | ConversationActivityEvent::WorkLifecycle {
                        source_message_ref,
                        invocation_ref,
                        ..
                    } => {
                        messages.contains(source_message_ref)
                            && published_work.contains(invocation_ref)
                    }
                    ConversationActivityEvent::WorkPhase { invocation_ref, .. } => {
                        published_work.contains(invocation_ref)
                    }
                    _ => true,
                }
        });
        self.append_message_work_associations(&mut conversation, "export", time);
        normalize_associated_work_sources(&mut conversation);
        // Association helpers can add public progress origins; keep only origins
        // with a disclosed source, rather than carrying private causal messages.
        conversation.activity.retain(|item| match &item.event {
            ConversationActivityEvent::WorkUpdate {
                source_message_ref, ..
            } => messages.contains(source_message_ref),
            _ => true,
        });
        let work = published_work
            .iter()
            .map(|reference| {
                let original = self
                    .invocation_record(reference)
                    .ok_or_else(|| invalid_error("Export publication work is missing"))?;
                let frozen = super::super::fork::snapshot::freeze(
                    original,
                    self.execution_conversation(reference),
                    &messages,
                    time,
                );
                let response = frozen
                    .terminal_response()
                    .ok_or_else(|| invalid_error("Export work has no settled response"))?;
                Ok(ExportedWork {
                    invocation_ref: reference.clone(),
                    requester_actor_ref: frozen.requester_actor_ref.clone(),
                    target_actor_ref: frozen.target_actor_ref.clone(),
                    action_name: frozen.action_name.clone(),
                    source_message_ref: frozen.source_message_ref.clone(),
                    reply_message_ref: frozen
                        .reply_message_ref
                        .clone()
                        .filter(|r| messages.contains(r)),
                    parent_invocation_ref: frozen
                        .parent_invocation_ref
                        .clone()
                        .filter(|r| published_work.contains(r)),
                    admission_sequence: frozen.admission_sequence,
                    admitted_at_unix_ms: frozen.admitted_at_unix_ms,
                    started_at_unix_ms: frozen.started_at_unix_ms,
                    finished_at_unix_ms: frozen.finished_at_unix_ms.unwrap_or(time),
                    status: response.status,
                    summary: response.summary.clone(),
                    activities: activities.remove(reference).unwrap_or_default(),
                })
            })
            .collect::<CwrResult<Vec<_>>>()?;
        let export = ConversationExport {
            version: EXPORT_VERSION_V3.into(),
            exported_at_unix_ms: time,
            conversation,
            timeline: Some(ConversationTimelineExport { timeline, work }),
        };
        validate_export(&export)?;
        Ok(export)
    }
}

impl ConversationTimelineExport {
    pub(super) fn validate(&self, export: &ConversationExport) -> CwrResult<()> {
        self.timeline
            .validate_for_conversation(&export.conversation.conversation_ref)?;
        let messages = export
            .conversation
            .messages
            .iter()
            .map(|m| m.message_ref.as_str())
            .collect::<BTreeSet<_>>();
        let mut work = BTreeSet::new();
        let mut activities = BTreeSet::new();
        for record in &self.work {
            for item in &record.activities {
                if item.activity.activity_ref.is_empty()
                    || item.first_sequence == 0
                    || item.first_sequence > item.sequence
                    || item.sequence > i64::MAX as u64
                    || !activities.insert(&item.activity.activity_ref)
                {
                    return invalid("Export has invalid inspector activity ordering");
                }
            }
            if record.invocation_ref.is_empty()
                || !work.insert(record.invocation_ref.as_str())
                || messages.contains(record.invocation_ref.as_str())
            {
                return invalid("Export has duplicate or invalid work identities");
            }
            if !record.status.is_terminal()
                || record
                    .status
                    .invocation_state()
                    .is_none_or(|state| state == ActionInvocationState::Stopping)
            {
                return invalid("Export work must be settled history");
            }
            if record
                .source_message_ref
                .as_deref()
                .is_some_and(|r| !messages.contains(r))
                || record
                    .reply_message_ref
                    .as_deref()
                    .is_some_and(|r| !messages.contains(r))
            {
                return invalid("Export work references a missing message");
            }
        }
        let parents = self
            .work
            .iter()
            .map(|r| {
                (
                    r.invocation_ref.as_str(),
                    r.parent_invocation_ref.as_deref(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut checked = BTreeSet::new();
        for record in &self.work {
            if record
                .parent_invocation_ref
                .as_deref()
                .is_some_and(|r| !work.contains(r))
            {
                return invalid("Export work references a missing parent");
            }
            let mut visited = BTreeSet::new();
            let mut current = Some(record.invocation_ref.as_str());
            while let Some(reference) = current {
                if checked.contains(reference) {
                    break;
                }
                if !visited.insert(reference) {
                    return invalid("Export work contains a parent cycle");
                }
                current = parents.get(reference).copied().flatten();
            }
            checked.extend(visited);
        }
        let mut placed_messages = BTreeSet::new();
        let mut placed_work = BTreeSet::new();
        for item in self.timeline.connections.iter().flat_map(|c| &c.items) {
            match &item.content {
                ConversationTimelineContent::Message { message_ref }
                    if messages.contains(message_ref.as_str()) =>
                {
                    placed_messages.insert(message_ref.as_str());
                }
                ConversationTimelineContent::Invocation { invocation_ref }
                    if work.contains(invocation_ref.as_str()) =>
                {
                    placed_work.insert(invocation_ref.as_str());
                }
                _ => return invalid("Export timeline references missing content"),
            }
        }
        if placed_messages != messages || placed_work != work {
            return invalid("Export contains unpublished content");
        }
        Ok(())
    }

    pub(super) fn plan(
        &self,
        export: &ConversationExport,
    ) -> CwrResult<super::super::fork::canonical::ForkTimeline> {
        validate_export(export)?;
        // Preserve relative call order without importing a foreign runtime's
        // global counter or allowing an input counter to exhaust local admission.
        let mut ordered = self.work.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|work| (work.admission_sequence, &work.invocation_ref));
        let records = ordered
            .into_iter()
            .enumerate()
            .map(|(index, work)| {
                work.record(&export.conversation.conversation_ref, index as u64 + 1)
            })
            .collect();
        let inspector = self
            .work
            .iter()
            .flat_map(|work| {
                work.activities.iter().map(move |item| {
                    let mut activity = item.activity.clone();
                    settle_activity(&mut activity);
                    super::super::tools::ToolOperation {
                        invocation_ref: work.invocation_ref.clone(),
                        attempt_ref: crate::cwpdb::derived_ref(
                            "export_tool_attempt",
                            &[&work.invocation_ref],
                        ),
                        provider: "retained_history".into(),
                        provider_item_id: activity.activity_ref.clone(),
                        host_call_id: None,
                        first_sequence: item.first_sequence,
                        sequence: item.sequence,
                        activity,
                    }
                })
            })
            .collect();
        Ok(super::super::fork::canonical::ForkTimeline::imported(
            self.timeline.clone(),
            records,
            inspector,
        ))
    }
}

impl ExportedWork {
    fn record(&self, conversation: &str, admission_sequence: u64) -> ActionInvocationRecord {
        ActionInvocationRecord {
            forked_from_invocation_ref: Some(self.invocation_ref.clone()),
            invocation_ref: self.invocation_ref.clone(),
            origin: InvocationOrigin::Ingress {
                integration_ref: conversation.into(),
                provider_event_ref: self.invocation_ref.clone(),
            },
            requester_actor_ref: self.requester_actor_ref.clone(),
            target_actor_ref: self.target_actor_ref.clone(),
            conversation_ref: Some(conversation.into()),
            actor_integration_ref: None,
            action_name: self.action_name.clone(),
            input: Value::Null,
            parent_invocation_ref: self.parent_invocation_ref.clone(),
            source_message_ref: self.source_message_ref.clone(),
            timeline_publications: BTreeMap::new(),
            state: self
                .status
                .invocation_state()
                .expect("validated settled status"),
            responses: vec![ActionResponse {
                progress: None,
                invocation_ref: self.invocation_ref.clone(),
                sequence: 1,
                status: self.status,
                summary: self.summary.clone(),
                result: self.status.is_completed().then_some(Value::Null),
                error: (self.status.is_failed() || self.status.is_blocked())
                    .then(|| ActionResponseDetail::new("Retained work failure.")),
                cause: None,
            }],
            reply_message_ref: self.reply_message_ref.clone(),
            admission_sequence,
            admitted_at_unix_ms: self.admitted_at_unix_ms,
            started_at_unix_ms: self.started_at_unix_ms,
            stop_requested_at_unix_ms: self.status.is_stopped().then_some(self.finished_at_unix_ms),
            stop_requested_by_actor_ref: None,
            stop_request_id: None,
            stop_reason: None,
            stop_execution_pending: false,
            stop_settlement_tracked: true,
            finished_at_unix_ms: Some(self.finished_at_unix_ms),
            attempt_count: u64::from(self.started_at_unix_ms.is_some()),
            attempt_started_at_unix_ms: None,
            attempt_started_after_activity_sequence: None,
            attempt_active: false,
        }
    }
}

// Canonical placements, including live streams and roots admitted through host
// APIs, are authoritative even when no legacy MessageRecorded event exists.
fn retain_message_boundaries(conversation: &mut RetainedConversation, time: u64) {
    let recorded = conversation
        .activity
        .iter()
        .filter_map(|item| match &item.event {
            ConversationActivityEvent::MessageRecorded { message_ref } => Some(message_ref.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    for message in &conversation.messages {
        if recorded.contains(&message.message_ref) {
            continue;
        }
        conversation.activity.push(ConversationActivity {
            activity_ref: crate::cwpdb::derived_ref(
                "export_message_boundary",
                &[&message.message_ref],
            ),
            sequence: conversation.activity.len() as u64 + 1,
            occurred_at_unix_ms: time,
            event: ConversationActivityEvent::MessageRecorded {
                message_ref: message.message_ref.clone(),
            },
        });
    }
}

fn settle_activity(activity: &mut WorkActivity) {
    if matches!(
        activity.state.as_str(),
        "running" | "pending" | "working" | "waiting" | "queued"
    ) {
        activity.state = "stopped".into();
    }
}
