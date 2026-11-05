use super::live_output::broadcast_changes;
use super::*;

pub(super) fn write_runtime_snapshot(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
) -> CwrResult<()> {
    let _broadcast = lock_broadcast(state)?;
    let events = {
        let mut runtime_state = lock_state(state)?;
        runtime_snapshot_events(
            &mut runtime_state,
            request_id,
            writer::uses_metadata_bootstrap(writer),
            writer::includes_fork_metadata(writer),
        )?
    };
    if let Err(error) = writer::try_write_events(writer, &events) {
        lock_state(state)?
            .subscribers
            .retain(|subscriber| !Arc::ptr_eq(subscriber, writer));
        retire_writer(writer);
        return Err(error);
    }
    Ok(())
}

pub(super) fn write_initial_runtime_snapshot(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
) -> CwrResult<()> {
    let _publication = lock_broadcast(state)?;
    let events = {
        let mut runtime_state = lock_state(state)?;
        runtime_snapshot_events(
            &mut runtime_state,
            request_id,
            writer::uses_metadata_bootstrap(writer),
            writer::includes_fork_metadata(writer),
        )?
    };
    if let Err(error) = writer::try_write_events(writer, &events) {
        retire_writer(writer);
        return Err(error);
    }
    let mut runtime_state = lock_state(state)?;
    if !runtime_state
        .subscribers
        .iter()
        .any(|subscriber| Arc::ptr_eq(subscriber, writer))
    {
        runtime_state.subscribers.push(Arc::clone(writer));
    }
    Ok(())
}

fn runtime_snapshot_events(
    state: &mut RuntimeServerState,
    request_id: String,
    metadata_only: bool,
    include_fork_metadata: bool,
) -> CwrResult<Vec<RuntimeServiceEvent>> {
    let runtime = state.runtime.clone();
    let mut conversations = if metadata_only {
        runtime
            .conversation_host()
            .bootstrap(include_fork_metadata)?
    } else {
        runtime.conversation_host().conversations()?
    };
    if !include_fork_metadata {
        strip_fork_metadata(&mut conversations);
    }
    let mut events = vec![RuntimeServiceEvent::ConversationSnapshot {
        conversations: conversations.clone(),
    }];
    let mut turns_by_conversation = runtime.conversation_host().turn_statuses()?;
    // Remember statuses seen through snapshots too. Keep earlier broadcast
    // values so the next publication still updates other connected clients.
    for (reference, turns) in &turns_by_conversation {
        state
            .published_turn_statuses
            .entry(reference.clone())
            .or_insert_with(|| turns.clone());
    }
    for conversation in conversations {
        let conversation_ref = conversation.conversation_ref;
        let turns = turns_by_conversation
            .remove(&conversation_ref)
            .unwrap_or_default();
        events.push(RuntimeServiceEvent::ConversationTurnStatusSnapshot {
            conversation_ref,
            turns,
        });
    }
    for (conversation_ref, statuses) in state.live_statuses.snapshot() {
        events.push(RuntimeServiceEvent::ConversationLiveStatusSnapshot {
            conversation_ref,
            statuses,
        });
    }
    let (required_changes, current_live_outputs) = state.live_outputs.broadcast_snapshot();
    for change in broadcast_changes(&BTreeMap::new(), &required_changes, &current_live_outputs) {
        events.push(RuntimeServiceEvent::ConversationLiveOutput {
            conversation_ref: change.conversation_ref,
            output: change.output,
        });
    }
    events.push(RuntimeServiceEvent::CommandSucceeded {
        request_id,
        admission: None,
        stopped_invocation_refs: Vec::new(),
        intrinsic_response: None,
        host_exchange: None,
    });
    Ok(events)
}

pub(super) fn strip_fork_metadata(
    conversations: &mut [cwr::v2026_09::conversation::RetainedConversation],
) {
    for conversation in conversations {
        conversation.activity.retain(|item| {
            !matches!(
                item.event,
                cwr::v2026_09::conversation::ConversationActivityEvent::ConversationForkedFrom {
                    ..
                } | cwr::v2026_09::conversation::ConversationActivityEvent::MessageWorkAssociation {
                    ..
                }
            )
        });
    }
}
