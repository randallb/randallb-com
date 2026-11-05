#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationTurnState {
    AwaitingPerson,
    Pending,
    Running,
    Waiting,
    Resuming,
}

pub use crate::adapter::ExecutionPhase as ConversationWorkerPhase;

/// One invocation currently contributing work to an unresolved root request.
///
/// The root request remains the control and stop scope. Workers are a separate
/// presentation projection so a host can name an executing descendant without
/// transferring control ownership to it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationTurnWorkerStatus {
    pub invocation_ref: String,
    pub target_actor_ref: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delegation_path_actor_refs: Vec<String>,
    pub state: ConversationTurnState,
    pub phase: ConversationWorkerPhase,
    pub admitted_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_activity_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_started_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_started_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_parts: Option<(u64, u64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationTurnStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<crate::adapter::user_input::UserInputQuestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_preview: Option<String>,
    pub invocation_ref: String,
    pub source_message_ref: String,
    pub requester_actor_ref: String,
    pub target_actor_ref: String,
    pub state: ConversationTurnState,
    pub admitted_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workers: Vec<ConversationTurnWorkerStatus>,
}

/// Projects every unresolved ingress root from CWR's invocation ledger.
///
/// Hosts present this state; they do not infer it from local commands, live
/// output, or provider activity.
pub fn project_conversation_turn_statuses(
    store: &ConversationAdmissionStore,
) -> BTreeMap<String, Vec<ConversationTurnStatus>> {
    let conversations = store
        .invocation_records()
        .filter(|invocation| {
            invocation.parent_invocation_ref.is_none()
                && matches!(invocation.origin, InvocationOrigin::Ingress { .. })
                && !invocation.state.is_terminal()
        })
        .filter_map(|invocation| invocation.conversation_ref.clone())
        .collect();
    let mut statuses = project_conversation_turn_statuses_for(store, &conversations);
    statuses.retain(|_, turns| !turns.is_empty());
    statuses
}

/// Projects only the selected conversations, including empty snapshots for
/// settled or unknown conversations. Other histories are not reconstructed.
pub fn project_conversation_turn_statuses_for(
    store: &ConversationAdmissionStore,
    conversation_refs: &BTreeSet<String>,
) -> BTreeMap<String, Vec<ConversationTurnStatus>> {
    conversation_refs
        .iter()
        .map(|reference| {
            (
                reference.clone(),
                project_turn_statuses_for_conversation(store, reference),
            )
        })
        .collect()
}

include!("turn_status/selection.rs");

#[cfg(any(test, feature = "test-read-metrics"))]
mod turn_status_measurement {
    use super::*;
    include!("turn_status/measurement.rs");
}
#[cfg(any(test, feature = "test-read-metrics"))]
pub use turn_status_measurement::measure_turn_status_projection;

fn project_turn_statuses_for_conversation(
    store: &ConversationAdmissionStore,
    conversation_ref: &str,
) -> Vec<ConversationTurnStatus> {
    let source_messages_by_invocation = store
        .admissions_for_conversation(conversation_ref)
        .filter_map(|admission| {
            admission
                .invocation_ref
                .as_deref()
                .map(|invocation_ref| (invocation_ref, admission.source_message_ref.as_str()))
        })
        .collect::<BTreeMap<_, _>>();
    let mut turns = Vec::new();
    let mut activity = None;
    for invocation in store.invocation_records().filter(|invocation| {
        invocation.conversation_ref.as_deref() == Some(conversation_ref)
            && invocation.parent_invocation_ref.is_none()
            && matches!(invocation.origin, InvocationOrigin::Ingress { .. })
            && !invocation.state.is_terminal()
    }) {
        let Some(source_message_ref) =
            source_messages_by_invocation.get(invocation.invocation_ref.as_str())
        else {
            continue;
        };
        let state = match invocation.state {
            ActionInvocationState::AwaitingPerson => ConversationTurnState::AwaitingPerson,
            ActionInvocationState::Admitted => ConversationTurnState::Pending,
            ActionInvocationState::Running => ConversationTurnState::Running,
            ActionInvocationState::Waiting => ConversationTurnState::Waiting,
            ActionInvocationState::Completed
            | ActionInvocationState::Blocked
            | ActionInvocationState::Failed
            | ActionInvocationState::Stopping
            | ActionInvocationState::Stopped => continue,
        };
        let activity = activity.get_or_insert_with(|| project_worker_activity(store, conversation_ref));
        turns.push(ConversationTurnStatus {
                questions: invocation
                    .input
                    .pointer("/provider_question/questions")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .unwrap_or_default()
                    .unwrap_or_default(),
                request_preview: (invocation.state == ActionInvocationState::AwaitingPerson).then(
                    || {
                        store
                            .conversation(conversation_ref)
                            .and_then(|c| {
                                c.messages
                                    .iter()
                                    .find(|m| m.message_ref == **source_message_ref)
                            })
                            .map(|m| {
                                let mut preview: String = m.text.chars().take(240).collect();
                                if m.text.chars().count() > 240 {
                                    preview.push('…');
                                }
                                preview
                            })
                            .unwrap_or_default()
                    },
                ),
                invocation_ref: invocation.invocation_ref.clone(),
                source_message_ref: (*source_message_ref).to_owned(),
                requester_actor_ref: invocation.requester_actor_ref.clone(),
                target_actor_ref: invocation.target_actor_ref.clone(),
                state,
                admitted_at_unix_ms: invocation.admitted_at_unix_ms,
                started_at_unix_ms: invocation.started_at_unix_ms,
                workers: project_turn_workers(store, invocation, activity),
            });
    }
    turns.sort_by(|left, right| {
        left.admitted_at_unix_ms
            .cmp(&right.admitted_at_unix_ms)
            .then_with(|| left.invocation_ref.cmp(&right.invocation_ref))
    });
    turns
}

struct WorkerActivity {
    latest_at: u64,
    phase: ConversationWorkerPhase,
    phase_started_at: u64,
    context_parts: Option<(u64, u64)>,
    turn_started_at: Option<u64>,
}

fn project_worker_activity(
    store: &ConversationAdmissionStore,
    conversation_ref: &str,
) -> BTreeMap<String, WorkerActivity> {
    #[cfg(any(test, feature = "test-read-metrics"))]
    turn_status_measurement::record(store, conversation_ref);
    let projected = store.conversation(conversation_ref)
        .map(|conversation| project_conversation_with_authoritative_work(conversation, store));
    let mut public_activity = BTreeMap::new();
    if let Some(projected) = projected {
        for item in projected.activity {
            let (invocation_ref, phase, parts) = match item.event {
                ConversationActivityEvent::WorkPhase { invocation_ref, activity, .. } => {
                    let phase = if activity.state == crate::adapter::ExecutionPhaseState::Completed {
                        ConversationWorkerPhase::Working
                    } else {
                        activity.phase
                    };
                    (
                        invocation_ref,
                        phase,
                        activity.completed_parts.zip(activity.total_parts),
                    )
                }
                ConversationActivityEvent::WorkUpdate {
                    invocation_ref,
                    status,
                    text,
                    activity,
                    ..
                } if status.is_progress() => {
                    let phase = if text.is_some() {
                        ConversationWorkerPhase::Responding
                    } else if let Some(activity) = activity.filter(|a| a.state == "running") {
                        if activity.child_invocation_ref.is_some() || activity.kind == "coworker" {
                            ConversationWorkerPhase::HandingOff
                        } else if activity.kind == "command" {
                            ConversationWorkerPhase::RunningCommand
                        } else {
                            ConversationWorkerPhase::UsingTool
                        }
                    } else {
                        continue;
                    };
                    (invocation_ref, phase, None)
                }
                _ => continue,
            };
            let entry = public_activity.entry(invocation_ref).or_insert(WorkerActivity {
                latest_at: 0,
                phase,
                phase_started_at: item.occurred_at_unix_ms,
                context_parts: parts,
                turn_started_at: (phase == ConversationWorkerPhase::Resuming)
                    .then_some(item.occurred_at_unix_ms),
            });
            if item.occurred_at_unix_ms >= entry.latest_at {
                if entry.phase != phase { entry.phase_started_at = item.occurred_at_unix_ms; }
                entry.latest_at = item.occurred_at_unix_ms;
                entry.phase = phase;
                entry.context_parts = parts;
                if phase == ConversationWorkerPhase::Resuming {
                    entry.turn_started_at = Some(item.occurred_at_unix_ms);
                }
            }
        }
    }
    public_activity
}

fn project_turn_workers(
    store: &ConversationAdmissionStore,
    root: &crate::v2026_09::invocation::ActionInvocationRecord,
    public_activity: &BTreeMap<String, WorkerActivity>,
) -> Vec<ConversationTurnWorkerStatus> {
    let records = store.invocation_records().collect::<Vec<_>>();
    let waits = store.invocation_waits();
    let mut workers = records
        .iter()
        .copied()
        .filter(|candidate| {
            !candidate.state.is_terminal()
                && invocation_belongs_to_root(candidate, root, &records)
                && !waiting_for_active_child(candidate, &records, waits)
                && !parent_waits_for_child(store, &candidate.invocation_ref)
        })
        .map(|worker| {
            let (state, default_phase) = worker_state_and_phase(worker, waits);
            // A pending or resumed wait overrides an older provider phase. A
            // completed child is a real event; repeated snapshots are not.
            let resumed_at = waits
                .iter()
                .filter(|wait| {
                    wait.waiter_invocation_ref == worker.invocation_ref
                        && matches!(
                            wait.state,
                            InvocationWaitState::Ready
                                | InvocationWaitState::Resuming
                                | InvocationWaitState::Consumed
                        )
                })
                .filter_map(|wait| {
                    records
                        .iter()
                        .find(|r| r.invocation_ref == wait.awaited_invocation_ref)
                        .and_then(|child| child.finished_at_unix_ms)
                })
                .max();
            let signal = public_activity.get(&worker.invocation_ref);
            let mut latest_activity_at_unix_ms = signal.map(|s| s.latest_at).or(worker.started_at_unix_ms);
            let mut phase = signal.map_or(default_phase, |s| s.phase);
            let mut phase_started_at_unix_ms = signal.map(|s| s.phase_started_at).or(worker.started_at_unix_ms);
            let mut context_parts = signal.and_then(|s| s.context_parts);
            let mut turn_started_at_unix_ms = signal
                .and_then(|s| s.turn_started_at)
                .or(worker.started_at_unix_ms);
            if worker.state == ActionInvocationState::Admitted || state == ConversationTurnState::Waiting {
                phase = default_phase;
                context_parts = None;
                turn_started_at_unix_ms = worker.started_at_unix_ms;
            } else if let Some(resumed) = resumed_at.filter(|at| signal.is_none_or(|s| *at >= s.latest_at)) {
                phase = ConversationWorkerPhase::Resuming;
                latest_activity_at_unix_ms = Some(resumed);
                phase_started_at_unix_ms = Some(resumed);
                context_parts = None;
                turn_started_at_unix_ms = Some(resumed);
            }
            ConversationTurnWorkerStatus {
                invocation_ref: worker.invocation_ref.clone(),
                target_actor_ref: worker.target_actor_ref.clone(),
                delegation_path_actor_refs: delegation_path_actor_refs(worker, root, &records),
                state,
                phase,
                admitted_at_unix_ms: worker.admitted_at_unix_ms,
                started_at_unix_ms: worker.started_at_unix_ms,
                latest_activity_at_unix_ms,
                phase_started_at_unix_ms,
                turn_started_at_unix_ms,
                context_parts,
            }
        })
        .collect::<Vec<_>>();
    workers.sort_by(|left, right| {
        worker_state_rank(left.state)
            .cmp(&worker_state_rank(right.state))
            .then_with(|| left.admitted_at_unix_ms.cmp(&right.admitted_at_unix_ms))
            .then_with(|| left.invocation_ref.cmp(&right.invocation_ref))
    });
    workers
}

fn delegation_path_actor_refs(
    worker: &crate::v2026_09::invocation::ActionInvocationRecord,
    root: &crate::v2026_09::invocation::ActionInvocationRecord,
    records: &[&crate::v2026_09::invocation::ActionInvocationRecord],
) -> Vec<String> {
    let mut path = Vec::new();
    let mut current = worker;
    while current.invocation_ref != root.invocation_ref {
        let Some(parent_ref) = current.parent_invocation_ref.as_deref() else {
            return Vec::new();
        };
        let Some(parent) = records
            .iter()
            .copied()
            .find(|record| record.invocation_ref == parent_ref)
        else {
            return Vec::new();
        };
        path.push(parent.target_actor_ref.clone());
        current = parent;
    }
    path.reverse();
    path
}

fn invocation_belongs_to_root(
    candidate: &crate::v2026_09::invocation::ActionInvocationRecord,
    root: &crate::v2026_09::invocation::ActionInvocationRecord,
    records: &[&crate::v2026_09::invocation::ActionInvocationRecord],
) -> bool {
    let mut current = candidate;
    loop {
        if current.invocation_ref == root.invocation_ref {
            return true;
        }
        let Some(parent_ref) = current.parent_invocation_ref.as_deref() else {
            return false;
        };
        let Some(parent) = records
            .iter()
            .copied()
            .find(|record| record.invocation_ref == parent_ref)
        else {
            return false;
        };
        current = parent;
    }
}

fn waiting_for_active_child(
    candidate: &crate::v2026_09::invocation::ActionInvocationRecord,
    records: &[&crate::v2026_09::invocation::ActionInvocationRecord],
    waits: &[crate::v2026_09::invocation::InvocationWait],
) -> bool {
    candidate.state == ActionInvocationState::Waiting
        && waits.iter().any(|wait| {
            wait.waiter_invocation_ref == candidate.invocation_ref
                && matches!(wait.state, InvocationWaitState::Pending)
                && records.iter().any(|record| {
                    record.invocation_ref == wait.awaited_invocation_ref
                        && !record.state.is_terminal()
                })
        })
}

fn worker_state_and_phase(
    worker: &crate::v2026_09::invocation::ActionInvocationRecord,
    waits: &[crate::v2026_09::invocation::InvocationWait],
) -> (ConversationTurnState, ConversationWorkerPhase) {
    match worker.state {
        ActionInvocationState::AwaitingPerson => (
            ConversationTurnState::AwaitingPerson,
            ConversationWorkerPhase::Waiting,
        ),
        ActionInvocationState::Admitted => (
            ConversationTurnState::Pending,
            ConversationWorkerPhase::Queued,
        ),
        ActionInvocationState::Running => (
            ConversationTurnState::Running,
            ConversationWorkerPhase::Working,
        ),
        ActionInvocationState::Waiting
            if waits.iter().any(|wait| {
                wait.waiter_invocation_ref == worker.invocation_ref
                    && matches!(
                        wait.state,
                        InvocationWaitState::Ready | InvocationWaitState::Resuming
                    )
            }) =>
        {
            (
                ConversationTurnState::Resuming,
                ConversationWorkerPhase::Resuming,
            )
        }
        ActionInvocationState::Waiting => (
            ConversationTurnState::Waiting,
            ConversationWorkerPhase::Waiting,
        ),
        ActionInvocationState::Completed
        | ActionInvocationState::Blocked
        | ActionInvocationState::Failed
        | ActionInvocationState::Stopping
        | ActionInvocationState::Stopped => unreachable!("terminal workers are filtered"),
    }
}

fn worker_state_rank(state: ConversationTurnState) -> u8 {
    match state {
        ConversationTurnState::Running | ConversationTurnState::Resuming => 0,
        ConversationTurnState::Waiting | ConversationTurnState::AwaitingPerson => 1,
        ConversationTurnState::Pending => 2,
    }
}
