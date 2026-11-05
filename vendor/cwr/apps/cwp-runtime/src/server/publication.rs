use super::*;
use cwr::v2026_09::conversation::ConversationActivityEvent;

#[cfg(test)]
pub(super) fn broadcast(state: &SharedState, event: RuntimeServiceEvent) -> CwrResult<()> {
    let _publication = lock_broadcast(state)?;
    broadcast_in_publication(state, event)
}

pub(super) fn broadcast_in_publication(
    state: &SharedState,
    event: RuntimeServiceEvent,
) -> CwrResult<()> {
    state.graphql_changes.send_replace(());
    let subscribers = lock_state(state)?.subscribers.clone();
    let failed = subscribers
        .into_iter()
        .filter(|subscriber| {
            let projected = project_subscriber_event(state, subscriber, &event);
            projected
                .and_then(|event| write_event(subscriber, &event))
                .is_err()
        })
        .collect::<Vec<_>>();
    if !failed.is_empty() {
        lock_state(state)?
            .subscribers
            .retain(|subscriber| !failed.iter().any(|failed| Arc::ptr_eq(subscriber, failed)));
    }
    for writer in failed {
        retire_writer(&writer);
    }
    Ok(())
}

pub(super) fn broadcast_conversation(
    state: &SharedState,
    runtime: &LocalRuntime,
    conversation_ref: &str,
) -> CwrResult<bool> {
    let _publication = lock_broadcast(state)?;
    let conversation = if needs_full_conversation_broadcast(state)? {
        runtime.conversation_host().conversation(conversation_ref)?
    } else {
        runtime.conversation_host().metadata(conversation_ref)?
    };
    if let Some(conversation) = conversation {
        broadcast_in_publication(
            state,
            RuntimeServiceEvent::ConversationUpsert {
                conversation: project_conversation(&conversation),
            },
        )?;
        return Ok(true);
    }
    Ok(false)
}

pub(super) fn needs_full_conversation_broadcast(state: &SharedState) -> CwrResult<bool> {
    Ok(lock_state(state)?
        .subscribers
        .iter()
        .any(|subscriber| !writer::uses_metadata_bootstrap(subscriber)))
}

pub(super) fn lock_broadcast(state: &SharedState) -> CwrResult<MutexGuard<'_, ()>> {
    state
        .broadcast_lock
        .lock()
        .map_err(|_| std::io::Error::other("runtime broadcast lock was poisoned").into())
}

fn project_subscriber_event(
    state: &SharedState,
    writer: &SharedWriter,
    event: &RuntimeServiceEvent,
) -> CwrResult<RuntimeServiceEvent> {
    if let RuntimeServiceEvent::ConversationUpsert { conversation } = event {
        if writer::uses_metadata_bootstrap(writer) {
            let mut conversation = LocalRuntime::conversation_metadata(conversation);
            if !writer::includes_fork_metadata(writer) {
                conversation.activity.retain(|item| {
                    !matches!(
                        item.event,
                        ConversationActivityEvent::ConversationForkedFrom { .. }
                            | ConversationActivityEvent::MessageWorkAssociation { .. }
                    )
                });
            }
            return Ok(RuntimeServiceEvent::ConversationUpsert { conversation });
        }
        let mut conversation = conversation.clone();
        conversation.activity.retain(|item| {
            !matches!(
                item.event,
                ConversationActivityEvent::ConversationForkedFrom { .. }
                    | ConversationActivityEvent::MessageWorkAssociation { .. }
            )
        });
        return Ok(RuntimeServiceEvent::ConversationUpsert { conversation });
    }
    let RuntimeServiceEvent::CatalogUpdated { catalog } = event else {
        return Ok(event.clone());
    };
    let state = lock_state(state)?;
    let owner = upstream_owner(writer);
    let actor = state
        .host_actors
        .get(&owner)
        .map(String::as_str)
        .unwrap_or(&catalog.acting_actor.actor_ref);
    Ok(RuntimeServiceEvent::CatalogUpdated {
        catalog: state.runtime.upstream_catalog_as(actor, Some(&owner))?,
    })
}
