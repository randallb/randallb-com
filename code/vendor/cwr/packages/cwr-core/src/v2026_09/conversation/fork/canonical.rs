//! Forks retain independent, non-executable copies of disclosed history.
use super::*;
use crate::v2026_09::invocation::{ActionInvocationRecord, InvocationOrigin};
use std::collections::BTreeSet;

pub(in crate::v2026_09::conversation) struct ForkTimeline {
    timeline: ConversationTimeline,
    records: Vec<ActionInvocationRecord>,
    inspector: Vec<ConversationActivity>,
    copied_inspector: Vec<super::super::tools::ToolOperation>,
}

impl ConversationAdmissionStore {
    pub(super) fn fork_canonical_conversation(
        &mut self,
        graph: &ResolvedCwrGraph,
        source: &RetainedConversation,
        caller: &str,
        message: &str,
        operation: &ForkOperation<'_>,
        timeline_item_ref: Option<&str>,
    ) -> CwrResult<RetainedConversation> {
        let mut timeline = self
            .retained_timeline(&source.conversation_ref)
            .ok_or_else(|| invalid_error("Fork source timeline is missing"))?
            .clone();
        let plan = super::boundary::ForkBoundary::resolve(
            self,
            source,
            &mut timeline,
            message,
            timeline_item_ref,
        )?;
        let disclosed = plan.messages.clone();
        let visual = Some(&plan);
        let (mut history, time) = self.build_fork_history_with_boundary(
            source,
            &source.conversation_ref,
            message,
            caller,
            &operation.fingerprint,
            visual,
        )?;
        history.timeline_connection_ref = None;
        history
            .messages
            .retain(|m| disclosed.contains(&m.message_ref));
        let messages = history
            .messages
            .iter()
            .map(|m| m.message_ref.clone())
            .collect::<BTreeSet<_>>();
        for message in &mut history.messages {
            if message
                .reply_to_message_ref
                .as_ref()
                .is_some_and(|r| !messages.contains(r))
            {
                message.reply_to_message_ref = None;
            }
        }
        history.activity.retain(|item| match &item.event {
            ConversationActivityEvent::MessageRecorded { message_ref }
            | ConversationActivityEvent::MessageWorkAssociation { message_ref, .. } => {
                messages.contains(message_ref)
            }
            ConversationActivityEvent::WorkLifecycle {
                source_message_ref, ..
            }
            | ConversationActivityEvent::WorkUpdate {
                source_message_ref, ..
            } => messages.contains(source_message_ref),
            _ => true,
        });
        super::super::transfer::normalize_associated_work_sources(&mut history);
        let mut records = Vec::new();
        let mut seen = BTreeSet::new();
        for item in timeline.connections.iter().flat_map(|c| &c.items) {
            let ConversationTimelineContent::Invocation { invocation_ref } = &item.content else {
                continue;
            };
            if !seen.insert(invocation_ref.clone()) {
                continue;
            }
            let original = self
                .invocation_record(invocation_ref)
                .ok_or_else(|| invalid_error("Fork publication invocation is missing"))?;
            let execution = plan.executions.iter().find(|execution| execution.activity.iter().any(|item|
                matches!(&item.event, ConversationActivityEvent::TurnStarted { invocation_ref: reference, .. } if reference == invocation_ref)));
            let record = super::snapshot::freeze(original, execution, &messages, time);
            records.push(record);
        }
        // Public work disclosure is independent of whether its causal message
        // has a placement in this prefix. Keep inspector facts out of portable
        // message history when that history cannot name their source.
        let disclosed_work = records
            .iter()
            .map(|record| record.invocation_ref.as_str())
            .collect::<BTreeSet<_>>();
        let inspector = plan.public_work.iter().filter(|item|
            matches!(&item.event, ConversationActivityEvent::WorkUpdate { invocation_ref, activity: Some(_), .. }
                if disclosed_work.contains(invocation_ref.as_str()) && self.invocation_record(invocation_ref).is_some_and(|record| record.forked_from_invocation_ref.is_none()))).cloned().collect();
        for item in &mut history.activity {
            if let ConversationActivityEvent::WorkLifecycle {
                invocation_ref,
                state,
                started_at_unix_ms,
                stop_requested_at_unix_ms,
                finished_at_unix_ms,
                ..
            } = &mut item.event
            {
                if let Some(record) = records
                    .iter()
                    .find(|record| &record.invocation_ref == invocation_ref)
                {
                    *state = record.state;
                    *started_at_unix_ms = record.started_at_unix_ms;
                    *stop_requested_at_unix_ms = record.stop_requested_at_unix_ms;
                    *finished_at_unix_ms = record.finished_at_unix_ms;
                }
            }
        }
        let copied_inspector = self
            .state
            .tool_operations
            .values()
            .filter(|operation| {
                disclosed_work.contains(operation.invocation_ref.as_str())
                    && self
                        .invocation_record(&operation.invocation_ref)
                        .is_some_and(|r| r.forked_from_invocation_ref.is_some())
            })
            .cloned()
            .collect();
        let reference = fork_conversation_ref(&self.state.runtime_root_ref, operation.id);
        self.import_history_to(
            graph,
            caller,
            ConversationExport {
                timeline: None,
                version: EXPORT_VERSION.into(),
                exported_at_unix_ms: time,
                conversation: history,
            },
            None,
            reference,
            Some(ForkTimeline {
                timeline,
                records,
                inspector,
                copied_inspector,
            }),
        )
    }
}

impl ForkTimeline {
    pub(in crate::v2026_09::conversation) fn imported(
        timeline: ConversationTimeline,
        records: Vec<ActionInvocationRecord>,
        copied_inspector: Vec<super::super::tools::ToolOperation>,
    ) -> Self {
        Self {
            timeline,
            records,
            inspector: Vec::new(),
            copied_inspector,
        }
    }

    pub(in crate::v2026_09::conversation) fn retain(
        self,
        state: &mut ConversationRuntimeState,
        conversation: &RetainedConversation,
    ) -> CwrResult<()> {
        let reference = &conversation.conversation_ref;
        let remap = |value: &str| format!("{reference}/history/{}", transfer_hash(value));
        let root = crate::cwpdb::derived_ref("fork_timeline_root", &[reference]);
        state.timeline_store.create(reference, &root)?;
        state
            .conversations
            .find_mut(|c| c.conversation_ref == *reference)
            .ok_or_else(|| invalid_error("Fork disappeared"))?
            .timeline_connection_ref = Some(root);
        let message_refs = conversation
            .messages
            .iter()
            .map(|m| m.message_ref.clone())
            .collect::<BTreeSet<_>>();
        let invocation_refs = self
            .records
            .iter()
            .map(|r| r.invocation_ref.clone())
            .collect::<BTreeSet<_>>();
        let mut provenance = BTreeMap::new();
        for item in self.timeline.connections.iter().flat_map(|c| &c.items) {
            if message_refs.contains(&remap(item.content.reference()))
                || invocation_refs.contains(item.content.reference())
            {
                provenance.insert(
                    remap(item.content.reference()),
                    item.content.reference().to_owned(),
                );
                provenance.insert(remap(&item.publication_ref), item.publication_ref.clone());
            }
        }
        if let Some(ConversationActivity {
            event: ConversationActivityEvent::ConversationForkedFrom { inherited_refs, .. },
            ..
        }) = state
            .conversations
            .find_mut(|c| c.conversation_ref == *reference)
            .and_then(|c| {
                c.activity.iter_mut().rev().find(|item| {
                    matches!(
                        item.event,
                        ConversationActivityEvent::ConversationForkedFrom { .. }
                    )
                })
            })
        {
            *inherited_refs = provenance;
        }
        let mut records = self
            .records
            .into_iter()
            .map(|mut record| {
                let original = record.invocation_ref.clone();
                record.invocation_ref = remap(&original);
                record.conversation_ref = Some(reference.clone());
                record.source_message_ref = record.source_message_ref.as_deref().map(&remap);
                record.reply_message_ref = record
                    .reply_message_ref
                    .as_deref()
                    .map(&remap)
                    .filter(|r| message_refs.contains(r));
                record.parent_invocation_ref = record
                    .parent_invocation_ref
                    .filter(|r| invocation_refs.contains(r))
                    .as_deref()
                    .map(&remap);
                record.origin = match &record.parent_invocation_ref {
                    Some(parent) => InvocationOrigin::HostCall {
                        caller_invocation_ref: parent.clone(),
                        call_id: original.clone(),
                    },
                    None => InvocationOrigin::Ingress {
                        integration_ref: reference.clone(),
                        provider_event_ref: original.clone(),
                    },
                };
                record.timeline_publications.clear();
                for response in &mut record.responses {
                    response.invocation_ref = record.invocation_ref.clone();
                }
                (original, record)
            })
            .collect::<BTreeMap<_, _>>();
        // Connections are traversed through ownership so children never precede parents.
        let mut pending = vec![(
            self.timeline.root_connection_ref.clone(),
            ConversationTimelineDisposition::Root,
        )];
        while let Some((connection_ref, disposition)) = pending.pop() {
            let connection = self
                .timeline
                .connection(&connection_ref)
                .ok_or_else(|| invalid_error("Fork connection is missing"))?;
            for item in &connection.items {
                let content = match &item.content {
                    ConversationTimelineContent::Message { message_ref }
                        if message_refs.contains(&remap(message_ref)) =>
                    {
                        ConversationTimelineContent::Message {
                            message_ref: remap(message_ref),
                        }
                    }
                    ConversationTimelineContent::Invocation { invocation_ref }
                        if records.contains_key(invocation_ref) =>
                    {
                        ConversationTimelineContent::Invocation {
                            invocation_ref: remap(invocation_ref),
                        }
                    }
                    _ => continue,
                };
                let publication = remap(&item.publication_ref);
                if let ConversationTimelineContent::Invocation { invocation_ref } = &item.content {
                    records
                        .get_mut(invocation_ref)
                        .ok_or_else(|| invalid_error("Fork work disappeared"))?
                        .timeline_publications
                        .insert(publication.clone(), disposition.clone());
                }
                let outcome = state.timeline_store.publish(
                    reference,
                    publication,
                    content,
                    disposition.clone(),
                )?;
                if let (Some(child), ConversationTimelinePublishOutcome::Placed(placed)) =
                    (&item.child_connection_ref, outcome)
                {
                    pending.push((
                        child.clone(),
                        ConversationTimelineDisposition::Child {
                            parent_item_ref: placed.item_ref,
                        },
                    ));
                }
            }
        }
        for (_, record) in records {
            if record.timeline_publications.is_empty() {
                return invalid("Fork work has no retained placement");
            }
            state.invocation_ledger.retain_fork_history(record)?;
        }
        for mut item in self.inspector {
            if let ConversationActivityEvent::WorkUpdate {
                invocation_ref,
                activity: Some(activity),
                ..
            } = &mut item.event
            {
                *invocation_ref = remap(invocation_ref);
                super::inspector::remap_activity(activity, &invocation_refs, &remap);
            }
            super::inspector::retain_items(state, std::iter::once(&item));
        }
        for operation in self.copied_inspector {
            let mut activity = operation.activity;
            super::inspector::remap_activity(&mut activity, &invocation_refs, &remap);
            super::inspector::retain_activity(
                state,
                &remap(&operation.invocation_ref),
                &activity,
                operation.first_sequence,
                operation.sequence,
            );
        }
        Ok(())
    }
}
