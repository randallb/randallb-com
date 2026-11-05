//! Provisional public messages use the same transaction and publication identities as final messages.
use super::*;
use crate::adapter::{AdapterClosedModelRun, AdapterLiveOutput};
use crate::open_responses::v2026_04_24::AssistantPhase;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationStreamingMessage {
    pub message: ConversationMessage,
    pub conversation_ref: String,
    pub invocation_ref: String,
    pub model_run_ref: String,
    pub phase: AssistantPhase,
    pub sequence: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct StreamingRun {
    pub model_run_ref: String,
    pub closed: bool,
    pub sequences: BTreeMap<String, i64>,
}

fn message_ref(
    runtime: &str,
    invocation: &str,
    run: &str,
    provider: &str,
    thread: Option<&str>,
    turn: &str,
    item: &str,
) -> String {
    let key = serde_json::to_string(&(invocation, run, provider, thread, turn, item))
        .expect("string tuple");
    format!(
        "{}/messages/stream-{}",
        runtime.trim_end_matches('/'),
        hex::encode(Sha256::digest(key.as_bytes()))
    )
}
impl ConversationAdmissionStore {
    /// Called by the existing model-output publisher; stale and closed runs cannot publish.
    pub fn record_streaming_output(
        &mut self,
        output: Option<&AdapterLiveOutput>,
        closed: &[AdapterClosedModelRun],
    ) -> CwrResult<bool> {
        self.transact(|state| {
            let mut changed = false;
            for run in closed {
                if let Some(current) = state.streaming_runs.get_mut(&run.invocation_ref) {
                    if current.model_run_ref == run.model_run_ref && !current.closed {
                        current.closed = true;
                        changed = true;
                    }
                }
            }
            if let Some(output) = output {
                changed |= stage_output(state, output)?;
            }
            Ok(changed)
        })
    }
}
fn stage_output(
    state: &mut ConversationRuntimeState,
    output: &AdapterLiveOutput,
) -> CwrResult<bool> {
    let Some(run) = state.streaming_runs.get(&output.invocation_ref) else {
        return Ok(false);
    };
    if run.closed
        || run.model_run_ref != output.model_run_ref
        || run
            .sequences
            .get(&output.response_or_turn_id)
            .is_some_and(|sequence| output.sequence_number <= *sequence)
    {
        return Ok(false);
    }
    let Some(invocation) = state.invocation_ledger.invocation(&output.invocation_ref) else {
        return Ok(false);
    };
    if invocation.state != crate::v2026_09::invocation::ActionInvocationState::Running
        || !invocation.attempt_active
    {
        return Ok(false);
    }
    let reference = message_ref(
        &state.runtime_root_ref,
        &output.invocation_ref,
        &output.model_run_ref,
        &output.provider,
        output.thread_id.as_deref(),
        &output.response_or_turn_id,
        &output.item_id,
    );
    if let Some(current) = state.streaming_messages.get_mut(&reference) {
        current.message.text = output.text.clone();
        current.phase = output.phase;
        current.sequence = output.sequence_number;
        state
            .streaming_runs
            .get_mut(&output.invocation_ref)
            .expect("checked run")
            .sequences
            .insert(output.response_or_turn_id.clone(), output.sequence_number);
        return Ok(true);
    }
    let (conversation_ref, source, author) =
        working::streaming_destination(state, &output.invocation_ref)?;
    let Some(timeline) = state.timeline_store.timeline(&conversation_ref) else {
        return Ok(false);
    };
    // Only an explicitly disclosed invocation may introduce public partial text.
    let parent = timeline.connections.iter().flat_map(|c| &c.items).find(|item|
        matches!(&item.content, ConversationTimelineContent::Invocation { invocation_ref } if invocation_ref == &output.invocation_ref))
        .map(|item| item.item_ref.clone());
    let Some(parent_item_ref) = parent else {
        return Ok(false);
    };

    if state
        .conversations
        .iter()
        .any(|c| c.messages.iter().any(|m| m.message_ref == reference))
    {
        return Ok(false);
    }
    // A changed provider item replaces an unpublished item, never a finalized message.
    let previous = state
        .streaming_messages
        .values()
        .filter(|m| m.invocation_ref == output.invocation_ref && m.message.message_ref != reference)
        .map(|m| m.message.message_ref.clone())
        .collect::<Vec<_>>();
    for reference in previous {
        withdraw(state, &reference)?;
    }
    let message = ConversationMessage {
        file_refs: Vec::new(),
        message_ref: reference.clone(),
        author_actor_ref: author,
        addressed_actor_refs: Vec::new(),
        addressing_mode: Some(ConversationAddressingMode::Implicit),
        text: output.text.clone(),
        visualizations: Vec::new(),
        reply_to_message_ref: source,
        action: None,
        progress: None,
    };
    state.streaming_messages.insert(
        reference.clone(),
        ConversationStreamingMessage {
            message,
            conversation_ref: conversation_ref.clone(),
            invocation_ref: output.invocation_ref.clone(),
            model_run_ref: output.model_run_ref.clone(),
            phase: output.phase,
            sequence: output.sequence_number,
        },
    );
    state
        .streaming_runs
        .get_mut(&output.invocation_ref)
        .expect("checked run")
        .sequences
        .insert(output.response_or_turn_id.clone(), output.sequence_number);
    state.timeline_store.publish(
        &conversation_ref,
        timeline::managed_publication_ref(&conversation_ref, "message", &reference),
        ConversationTimelineContent::Message {
            message_ref: reference,
        },
        ConversationTimelineDisposition::Child { parent_item_ref },
    )?;
    Ok(true)
}
fn withdraw(state: &mut ConversationRuntimeState, reference: &str) -> CwrResult<()> {
    if let Some(message) = state.streaming_messages.remove(reference) {
        state
            .timeline_store
            .withdraw_streaming(&message.conversation_ref, reference)?;
    }
    Ok(())
}
pub(super) fn withdraw_inactive(state: &mut ConversationRuntimeState) -> CwrResult<()> {
    let stale = state
        .streaming_messages
        .values()
        .filter(|message| {
            state
                .invocation_ledger
                .invocation(&message.invocation_ref)
                .is_none_or(|invocation| {
                    invocation.state != crate::v2026_09::invocation::ActionInvocationState::Running
                        || !invocation.attempt_active
                })
                || state
                    .streaming_runs
                    .get(&message.invocation_ref)
                    .is_none_or(|run| run.model_run_ref != message.model_run_ref)
        })
        .map(|message| message.message.message_ref.clone())
        .collect::<Vec<_>>();
    for reference in stale {
        withdraw(state, &reference)?;
    }
    let finished = state
        .streaming_runs
        .keys()
        .filter(|reference| {
            state
                .invocation_ledger
                .invocation(reference)
                .is_none_or(|invocation| {
                    invocation.state != crate::v2026_09::invocation::ActionInvocationState::Running
                        || !invocation.attempt_active
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    for reference in finished {
        state.streaming_runs.remove(&reference);
    }
    Ok(())
}
pub(super) fn withdraw_on_restart(state: &mut ConversationRuntimeState) -> CwrResult<bool> {
    let references = state.streaming_messages.keys().cloned().collect::<Vec<_>>();
    let changed = !references.is_empty() || !state.streaming_runs.is_empty();
    for reference in references {
        withdraw(state, &reference)?;
    }
    for reference in state.streaming_runs.keys().cloned().collect::<Vec<_>>() {
        state.streaming_runs.remove(&reference);
    }
    Ok(changed)
}
pub(super) fn finalize_commentary(
    state: &mut ConversationRuntimeState,
    invocation: &str,
    activity: &ConversationActivity,
) -> CwrResult<Option<String>> {
    let ConversationActivityEvent::CommentaryProgress {
        model_run_ref,
        provider,
        thread_id,
        response_or_turn_id,
        item_id,
        ..
    } = &activity.event
    else {
        return Ok(None);
    };
    // Derive the same identity even if the final event wins the race with the first preview poll.
    let reference = message_ref(
        &state.runtime_root_ref,
        invocation,
        model_run_ref,
        provider,
        thread_id.as_deref(),
        response_or_turn_id,
        item_id,
    );
    state.streaming_messages.remove(&reference);
    Ok(Some(reference))
}
pub(super) fn finalize_answer(
    state: &mut ConversationRuntimeState,
    invocation: &str,
    completed: bool,
) -> Option<String> {
    if !completed {
        return None;
    }
    let reference = state
        .streaming_messages
        .values()
        .find(|m| m.invocation_ref == invocation && m.phase == AssistantPhase::FinalAnswer)
        .map(|m| m.message.message_ref.clone())?;
    state.streaming_messages.remove(&reference);
    Some(reference)
}
