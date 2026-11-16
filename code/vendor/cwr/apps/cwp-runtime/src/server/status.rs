use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::*;

pub(super) fn broadcast_turn_statuses(
    state: &SharedState,
    runtime: &LocalRuntime,
) -> CwrResult<()> {
    broadcast_turn_statuses_for(state, runtime, &std::collections::BTreeSet::new())
}

pub(super) fn broadcast_turn_statuses_for(
    state: &SharedState,
    runtime: &LocalRuntime,
    force: &std::collections::BTreeSet<String>,
) -> CwrResult<()> {
    let _publication = lock_broadcast(state)?;
    let current = runtime.conversation_host().turn_statuses()?;
    publish_full_turn_statuses_in_publication(state, current, force)
}

type TurnStatusMap = BTreeMap<String, Vec<cwr::v2026_09::host::ConversationTurnStatus>>;

fn publish_full_turn_statuses_in_publication(
    state: &SharedState,
    current: TurnStatusMap,
    force: &std::collections::BTreeSet<String>,
) -> CwrResult<()> {
    let previous = std::mem::replace(
        &mut lock_state(state)?.published_turn_statuses,
        current.clone(),
    );
    let mut changed = current
        .iter()
        .filter(|(reference, turns)| previous.get(*reference) != Some(*turns))
        .map(|(reference, turns)| (reference.clone(), turns.clone()))
        .chain(
            previous
                .keys()
                .filter(|reference| !current.contains_key(*reference))
                .map(|reference| (reference.clone(), Vec::new())),
        )
        .collect::<BTreeMap<_, _>>();
    for reference in force {
        changed.insert(
            reference.clone(),
            current.get(reference).cloned().unwrap_or_default(),
        );
    }
    for (conversation_ref, turns) in changed {
        broadcast_in_publication(
            state,
            RuntimeServiceEvent::ConversationTurnStatusSnapshot {
                conversation_ref,
                turns,
            },
        )?;
    }
    Ok(())
}

/// Capture affected status and all consulted activity cursors atomically, then
/// publish under the existing ordering lock. A nested worker also checks its
/// ancestors for admissions or controls retained between execution callbacks.
pub(super) fn broadcast_execution_turn_statuses<'a>(
    state: &SharedState,
    runtime: &LocalRuntime,
    invocations: impl IntoIterator<Item = &'a str>,
    conversation_ref: &str,
    published_activity: &BTreeMap<String, Option<String>>,
) -> CwrResult<BTreeMap<String, Option<String>>> {
    let _publication = lock_broadcast(state)?;
    let publication = runtime
        .conversation_host()
        .turn_status_publication_for_execution(
            &invocations.into_iter().map(str::to_owned).collect(),
            &std::collections::BTreeSet::from([conversation_ref.to_owned()]),
            published_activity,
        )?;
    if publication.replace_all {
        publish_full_turn_statuses_in_publication(
            state,
            publication.statuses,
            &std::collections::BTreeSet::new(),
        )?;
    } else {
        publish_scoped_turn_statuses_in_publication(state, publication.statuses)?;
    }
    Ok(publication.activity_cursors)
}

fn publish_scoped_turn_statuses_in_publication(
    state: &SharedState,
    current: TurnStatusMap,
) -> CwrResult<()> {
    let mut changed = BTreeMap::new();
    {
        let mut state = lock_state(state)?;
        for (reference, turns) in current {
            let previous = state.published_turn_statuses.get(&reference);
            if previous.is_some_and(|previous| previous != &turns)
                || (previous.is_none() && !turns.is_empty())
            {
                changed.insert(reference.clone(), turns.clone());
            }
            if turns.is_empty() {
                state.published_turn_statuses.remove(&reference);
            } else {
                state.published_turn_statuses.insert(reference, turns);
            }
        }
    }
    for (conversation_ref, turns) in changed {
        broadcast_in_publication(
            state,
            RuntimeServiceEvent::ConversationTurnStatusSnapshot {
                conversation_ref,
                turns,
            },
        )?;
    }
    Ok(())
}

pub(super) fn start_live_status_expiry(state: SharedState) -> CwrResult<()> {
    thread::Builder::new()
        .name("cwp-runtime-live-status".into())
        .spawn(move || loop {
            thread::sleep(Duration::from_millis(250));
            let _publication = match lock_broadcast(&state) {
                Ok(publication) => publication,
                Err(_) => return,
            };
            let changed = match lock_state(&state) {
                Ok(mut state) => state.live_statuses.expire(current_unix_ms()),
                Err(_) => return,
            };
            for (conversation_ref, statuses) in changed {
                let _ = broadcast_in_publication(
                    &state,
                    RuntimeServiceEvent::ConversationLiveStatusSnapshot {
                        conversation_ref,
                        statuses,
                    },
                );
            }
        })?;
    Ok(())
}

pub(super) fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub(super) fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(std::io::Error::new(std::io::ErrorKind::InvalidData, message.into()).into())
}
