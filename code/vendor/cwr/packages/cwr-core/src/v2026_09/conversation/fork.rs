//! Fork contract
//!
//! A canonical fork selects one published message placement. Its history is the
//! depth-first visual prefix through that placement, inclusive. Repeated
//! placements require an explicit item only when the selected message repeats.
//! Persisted order of every retained publication and the work events producing
//! disclosed replies determines the cutoff; timestamps never determine membership.
//!
//! Messages retain authorship and links to other copied messages. A disclosed
//! invocation remains visible even if its source message is undisclosed; that
//! copy has no source link. Work facts come from the cut-off public history,
//! rather than current provider records. Unfinished work becomes stopped history
//! and can neither execute nor create completion attention in the destination.
//! The destination owns new identities and records its source lineage. When
//! forking an existing fork, its published invocation snapshots stay immutable;
//! selecting an earlier message trims placements without rewinding those snapshots.
//!
//! The shared action lifecycle owns admission and retry identity. A retry recovers
//! the same committed destination, including after restart. Each Desktop environment
//! owns one unresolved fork choice per conversation and identity; uncertain failures keep
//! its request ID, while definitive rejection permits a new choice. Opening the
//! destination can retry without creating another conversation.

use super::transfer::{public_activity, transfer_hash, ConversationExport, EXPORT_VERSION};
use super::*;
use crate::v2026_09::invocation::ActionInvocationState;

mod boundary;
pub(super) mod canonical;
mod inspector;
pub(in crate::v2026_09::conversation) mod snapshot;

/// Destination identity never depends on mutable retry arguments. The retained
/// fingerprint binds that identity to one exact placement selection.
struct ForkOperation<'a> {
    id: &'a str,
    fingerprint: String,
}

impl<'a> ForkOperation<'a> {
    fn new(id: &'a str, placement: Option<&str>) -> CwrResult<Self> {
        let fingerprint = match placement {
            Some(item) => serde_json::to_string(&(id, item))?,
            None => id.to_owned(),
        };
        Ok(Self { id, fingerprint })
    }
}

impl ConversationAdmissionStore {
    fn forked_conversation(
        &self,
        caller: &str,
        operation: &ForkOperation<'_>,
        source_conversation_ref: &str,
        source_message_ref: &str,
    ) -> CwrResult<Option<RetainedConversation>> {
        let reference = fork_conversation_ref(&self.state.runtime_root_ref, operation.id);
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
        let matches_request = conversation.activity.iter().any(|item| {
            matches!(
                &item.event,
                ConversationActivityEvent::ConversationForkedFrom {
                    source_conversation_ref: retained_conversation,
                    source_message_ref: retained_message,
                    forked_by_actor_ref,
                    operation_id: retained_operation,
                    ..
                } if retained_conversation == source_conversation_ref
                    && retained_message == source_message_ref
                    && forked_by_actor_ref == caller
                    && retained_operation == &operation.fingerprint
            )
        });
        if !matches_request {
            return invalid("Fork ID was already used with different arguments");
        }
        Ok(Some(conversation))
    }

    pub fn fork_conversation(
        &mut self,
        graph: &ResolvedCwrGraph,
        caller: &str,
        source_conversation_ref: &str,
        source_message_ref: &str,
        fork_id: &str,
    ) -> CwrResult<RetainedConversation> {
        self.fork_conversation_at(
            graph,
            caller,
            source_conversation_ref,
            source_message_ref,
            fork_id,
            None,
        )
    }

    pub fn fork_conversation_at(
        &mut self,
        graph: &ResolvedCwrGraph,
        caller: &str,
        source_conversation_ref: &str,
        source_message_ref: &str,
        fork_id: &str,
        timeline_item_ref: Option<&str>,
    ) -> CwrResult<RetainedConversation> {
        self.hydrate_all_cold_conversations()?;
        let operation = ForkOperation::new(fork_id, timeline_item_ref)?;
        if !graph.actors.iter().any(|actor| actor.actor_ref == caller) {
            return invalid("Fork requires a coworker in this workspace");
        }
        if let Some(existing) = self.forked_conversation(
            caller,
            &operation,
            source_conversation_ref,
            source_message_ref,
        )? {
            return Ok(existing);
        }
        let source = self
            .conversation(source_conversation_ref)
            .ok_or_else(|| invalid_error("Fork source conversation was not found"))?
            .clone();
        if !source
            .participant_actor_refs
            .iter()
            .any(|actor| actor == caller)
        {
            return invalid("Fork requires source conversation participation");
        }
        if source.parent_conversation_ref.is_some() {
            return invalid("Only shared conversations can be forked");
        }
        if source.timeline_connection_ref.is_some() {
            return self.fork_canonical_conversation(
                graph,
                &source,
                caller,
                source_message_ref,
                &operation,
                timeline_item_ref,
            );
        }
        if timeline_item_ref.is_some() {
            return invalid("The selected conversation has no canonical timeline");
        }
        let (history, forked_at_unix_ms) = self.build_fork_history(
            &source,
            source_conversation_ref,
            source_message_ref,
            caller,
            &operation.fingerprint,
        )?;
        self.import_conversation_to(
            graph,
            caller,
            ConversationExport {
                timeline: None,
                version: EXPORT_VERSION.into(),
                exported_at_unix_ms: forked_at_unix_ms,
                conversation: history,
            },
            None,
            fork_conversation_ref(&self.state.runtime_root_ref, fork_id),
        )
    }

    fn build_fork_history(
        &self,
        source: &RetainedConversation,
        source_conversation_ref: &str,
        source_message_ref: &str,
        caller: &str,
        fork_id: &str,
    ) -> CwrResult<(RetainedConversation, u64)> {
        self.build_fork_history_with_boundary(
            source,
            source_conversation_ref,
            source_message_ref,
            caller,
            fork_id,
            None,
        )
    }

    fn build_fork_history_with_boundary(
        &self,
        source: &RetainedConversation,
        source_conversation_ref: &str,
        source_message_ref: &str,
        caller: &str,
        fork_id: &str,
        visual: Option<&boundary::ForkBoundary>,
    ) -> CwrResult<(RetainedConversation, u64)> {
        let boundary_activity = source
            .activity
            .iter()
            .find_map(|item| match &item.event {
                ConversationActivityEvent::MessageRecorded { message_ref }
                    if message_ref == source_message_ref =>
                {
                    Some(item.clone())
                }
                _ => None,
            })
            .ok_or_else(|| invalid_error("Fork message was not found in retained history"))?;
        let mut boundary_order = self
            .retained_ref_order(&boundary_activity.activity_ref)?
            .ok_or_else(|| invalid_error("Fork message boundary was not persisted"))?;
        let selected_message = source
            .messages
            .iter()
            .find(|message| message.message_ref == source_message_ref)
            .expect("message activity references a retained message");
        if matches!(
            selected_message.action,
            Some(ConversationActionEvent::Call { .. })
        ) {
            return invalid("Forking from an action-call message is not supported");
        }
        let selected_activity_refs = self.selected_message_activity_refs();
        if let Some(associated_ref) = selected_activity_refs.get(source_message_ref) {
            boundary_order = boundary_order.max(
                self.retained_ref_order(associated_ref)?
                    .ok_or_else(|| invalid_error("Fork message activity was not persisted"))?,
            );
        }
        let selected_message_index = source
            .messages
            .iter()
            .position(|message| message.message_ref == source_message_ref)
            .expect("selected message was found above");
        let default_message_refs = source
            .messages
            .iter()
            .take(selected_message_index + 1)
            .map(|message| message.message_ref.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let retained_message_refs = if let Some(plan) = visual {
            boundary_order = plan.order;
            plan.messages.clone()
        } else {
            default_message_refs
        };
        let forked_at_unix_ms = current_unix_ms();
        let mut history = crate::v2026_09::host::project_conversation(source);
        if let Some(plan) = visual {
            history.activity.extend(plan.public_work.clone());
        } else {
            let executions = self.boundary_executions(source_conversation_ref, boundary_order)?;
            history
                .activity
                .extend(snapshot::public_work(self, source, &executions));
        }
        crate::v2026_09::host::sort_work_activities(&mut history.activity);
        history.archived = false;
        history
            .messages
            .retain(|message| retained_message_refs.contains(&message.message_ref));
        let activity_by_ref = history
            .activity
            .iter()
            .enumerate()
            .map(|(index, activity)| (activity.activity_ref.as_str(), index))
            .collect::<BTreeMap<_, _>>();
        let mut retained_work_boundaries = history
            .messages
            .iter()
            .filter_map(|message| {
                let activity_ref = selected_activity_refs.get(&message.message_ref)?;
                let activity = &history.activity[*activity_by_ref.get(activity_ref.as_str())?];
                let ConversationActivityEvent::WorkUpdate { invocation_ref, .. } = &activity.event
                else {
                    return None;
                };
                let boundary = Some(self.retained_ref_order(activity_ref).ok().flatten()?);
                Some((invocation_ref.clone(), boundary))
            })
            .fold(
                BTreeMap::<String, Option<i64>>::new(),
                |mut boundaries, (invocation_ref, boundary)| {
                    boundaries
                        .entry(invocation_ref)
                        .and_modify(|retained| {
                            *retained = match (*retained, boundary) {
                                (None, _) | (_, None) => None,
                                (Some(left), Some(right)) => Some(left.max(right)),
                            };
                        })
                        .or_insert(boundary);
                    boundaries
                },
            );
        self.retain_work_without_replies(
            &history,
            &retained_message_refs,
            boundary_order,
            &mut retained_work_boundaries,
        )?;
        let parent_by_invocation = history
            .activity
            .iter()
            .filter_map(|item| match &item.event {
                ConversationActivityEvent::WorkLifecycle {
                    invocation_ref,
                    parent_invocation_ref: Some(parent),
                    ..
                }
                | ConversationActivityEvent::WorkUpdate {
                    invocation_ref,
                    parent_invocation_ref: Some(parent),
                    ..
                } => Some((invocation_ref.clone(), parent.clone())),
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();
        let retained_invocations = retained_work_boundaries.keys().cloned().collect::<Vec<_>>();
        for invocation_ref in retained_invocations {
            let mut current = invocation_ref;
            let mut visited = std::collections::BTreeSet::new();
            while let Some(parent) = parent_by_invocation.get(&current) {
                if !visited.insert(parent.clone()) {
                    break;
                }
                retained_work_boundaries
                    .entry(parent.clone())
                    .and_modify(|retained| {
                        *retained = retained.map(|left| left.max(boundary_order));
                    })
                    .or_insert(Some(boundary_order));
                current = parent.clone();
            }
        }
        self.retain_fork_activity(
            &mut history,
            &retained_message_refs,
            &retained_work_boundaries,
            forked_at_unix_ms,
        );
        history.messages = history
            .messages
            .iter()
            .map(ConversationMessage::clone_for_external_delivery)
            .collect();
        self.append_message_work_associations(
            &mut history,
            &format!("forks/{}", transfer_hash(fork_id)),
            forked_at_unix_ms,
        );
        let lineage_depth = fork_lineage_depth(source);
        history.activity.push(ConversationActivity {
            activity_ref: format!(
                "{source_conversation_ref}/forks/{}/lineage",
                transfer_hash(fork_id)
            ),
            sequence: history.activity.len() as u64 + 1,
            occurred_at_unix_ms: forked_at_unix_ms,
            event: ConversationActivityEvent::ConversationForkedFrom {
                inherited_refs: Default::default(),
                source_conversation_ref: source_conversation_ref.into(),
                source_message_ref: source_message_ref.into(),
                forked_by_actor_ref: caller.into(),
                operation_id: fork_id.into(),
                forked_at_unix_ms,
                lineage_depth,
            },
        });
        Ok((history, forked_at_unix_ms))
    }

    fn boundary_executions(
        &self,
        source_conversation_ref: &str,
        boundary_order: i64,
    ) -> CwrResult<Vec<RetainedConversation>> {
        let mut boundary_executions = Vec::new();
        for execution in self.conversations().filter(|candidate| {
            candidate.parent_conversation_ref.as_deref() == Some(source_conversation_ref)
        }) {
            let mut retained = execution.clone();
            let mut activity = Vec::new();
            for item in retained.activity {
                if self
                    .retained_ref_order(&item.activity_ref)?
                    .is_some_and(|order| order <= boundary_order)
                {
                    activity.push(item);
                }
            }
            retained.activity = activity;
            boundary_executions.push(retained);
        }
        Ok(boundary_executions)
    }

    fn retain_work_without_replies(
        &self,
        history: &RetainedConversation,
        retained_message_refs: &std::collections::BTreeSet<String>,
        boundary_order: i64,
        retained_work_boundaries: &mut BTreeMap<String, Option<i64>>,
    ) -> CwrResult<()> {
        // Root work can have no reply yet, or intentionally have no reply when
        // it fails or stops. Retain it through the selected boundary even when
        // no message/work association can reach it.
        for item in &history.activity {
            let ConversationActivityEvent::WorkUpdate {
                invocation_ref,
                source_message_ref,
                ..
            } = &item.event
            else {
                continue;
            };
            if !retained_message_refs.contains(source_message_ref) {
                continue;
            }
            let Some(order) = self
                .retained_ref_order(&item.activity_ref)?
                .filter(|order| *order <= boundary_order)
            else {
                continue;
            };
            // A terminal update still has a position. Later legacy updates for
            // the same invocation must not cross the selected message boundary.
            let boundary = Some(order);
            retained_work_boundaries
                .entry(invocation_ref.clone())
                .and_modify(|retained| {
                    *retained = match (*retained, boundary) {
                        (None, _) | (_, None) => None,
                        (Some(left), Some(right)) => Some(left.max(right)),
                    };
                })
                .or_insert(boundary);
        }
        Ok(())
    }

    fn retain_fork_activity(
        &self,
        history: &mut RetainedConversation,
        retained_message_refs: &std::collections::BTreeSet<String>,
        retained_work_boundaries: &BTreeMap<String, Option<i64>>,
        forked_at_unix_ms: u64,
    ) {
        let mut latest_work_status = BTreeMap::<String, (i64, ActionResponseStatus)>::new();
        history.activity.retain(|item| {
            if !public_activity(&item.event) {
                return false;
            }
            match &item.event {
                ConversationActivityEvent::MessageRecorded { message_ref } => {
                    retained_message_refs.contains(message_ref)
                }
                // Lineage describes the copied conversation rather than an event
                // after its last message, so inherited ancestry survives any
                // message boundary selected inside that copied history.
                ConversationActivityEvent::ConversationForkedFrom { .. } => true,
                ConversationActivityEvent::MessageWorkAssociation { .. } => false,
                ConversationActivityEvent::WorkUpdate {
                    invocation_ref,
                    source_message_ref,
                    status,
                    ..
                } => {
                    if !retained_message_refs.contains(source_message_ref) {
                        return false;
                    }
                    let Some(boundary) = retained_work_boundaries.get(invocation_ref) else {
                        return false;
                    };
                    let order = self.retained_ref_order(&item.activity_ref).ok().flatten();
                    let Some(order) = order.filter(|order| {
                        boundary.is_none_or(|boundary_order| *order <= boundary_order)
                    }) else {
                        return false;
                    };
                    let latest = latest_work_status
                        .entry(invocation_ref.clone())
                        .or_insert((order, *status));
                    if (!latest.1.is_terminal() && status.is_terminal())
                        || (latest.1.is_terminal() == status.is_terminal() && order >= latest.0)
                    {
                        *latest = (order, *status);
                    }
                    true
                }
                ConversationActivityEvent::WorkPhase { invocation_ref, .. } => {
                    let Some(boundary) = retained_work_boundaries.get(invocation_ref) else {
                        return false;
                    };
                    self.retained_ref_order(
                        item.activity_ref
                            .strip_suffix("/phase")
                            .unwrap_or(&item.activity_ref),
                    )
                    .ok()
                    .flatten()
                    .is_some_and(|order| {
                        boundary.is_none_or(|boundary_order| order <= boundary_order)
                    })
                }
                ConversationActivityEvent::WorkLifecycle {
                    source_message_ref, ..
                } => retained_message_refs.contains(source_message_ref),
                _ => false,
            }
        });
        history.activity.retain_mut(|item| {
            let ConversationActivityEvent::WorkLifecycle {
                invocation_ref,
                state,
                stop_requested_at_unix_ms,
                finished_at_unix_ms,
                ..
            } = &mut item.event
            else {
                return true;
            };
            let Some((_, status)) = latest_work_status.get(invocation_ref) else {
                return false;
            };
            *state = status
                .invocation_state()
                .unwrap_or(ActionInvocationState::Stopped);
            if !state.is_terminal() {
                *stop_requested_at_unix_ms = None;
                *finished_at_unix_ms = None;
            } else if *state == ActionInvocationState::Stopped && !status.is_terminal() {
                // A copied nonterminal update cannot complete in the fork, so
                // represent it as stopped at the fork boundary. A genuinely
                // stopped invocation already has its historical timestamps.
                *stop_requested_at_unix_ms = None;
                *finished_at_unix_ms = Some(forked_at_unix_ms);
            }
            true
        });
    }

    fn retained_ref_order(&self, reference: &str) -> CwrResult<Option<i64>> {
        let mut backend = self
            .backend
            .lock()
            .map_err(|_| invalid_error("conversation backend lock poisoned"))?;
        backend.retained_order(reference)
    }
}

fn fork_lineage_depth(source: &RetainedConversation) -> u64 {
    source
        .activity
        .iter()
        .filter_map(|item| match item.event {
            ConversationActivityEvent::ConversationForkedFrom { lineage_depth, .. } => {
                Some(lineage_depth)
            }
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn fork_conversation_ref(runtime_root_ref: &str, fork_id: &str) -> String {
    format!(
        "{runtime_root_ref}/conversations/fork-{}",
        transfer_hash(fork_id)
    )
}
