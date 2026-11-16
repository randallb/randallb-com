use super::*;

pub(in crate::server) struct ArchiveControl {
    writer: SharedWriter,
    request_id: String,
    operation_id: Option<String>,
    command: IntrinsicCwrCommand,
    requesting_actor_ref: String,
    conversation_ref: Option<String>,
    reservation: Arc<cwr::v2026_09::local::LocalArchiveReservation>,
    _activity: crate::idle::Consumer,
    _delivery: crate::idle::Delivery,
}

// One worker drains each conversation queue in command-reader order. Queued
// controls occupy no threads; provider acknowledgements still pass the reader.
pub(in crate::server) fn execute_archive_in_background(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    operation_id: Option<String>,
    command: IntrinsicCwrCommand,
    requesting_actor_ref: String,
    conversation_ref: Option<String>,
) -> CwrResult<()> {
    let activity = state
        .activity
        .connect()
        .ok_or_else(|| std::io::Error::other("runtime is closing"))?;
    let conversation = conversation_ref.clone().unwrap_or_default();
    let mut locked = lock_state(state)?;
    // Reject excess controls before retaining writers, guards, or ordering tickets.
    if (!locked.conversation_controls.contains_key(&conversation)
        && locked.conversation_controls.len() >= 256)
        || locked
            .conversation_controls
            .get(&conversation)
            .is_some_and(|queue| queue.len() >= 64)
        || locked
            .conversation_controls
            .values()
            .map(|queue| queue.len())
            .sum::<usize>()
            >= 256
    {
        return Err(std::io::Error::other(
            "conversation control queue is full; retry after pending controls finish",
        )
        .into());
    }
    let reservation = locked
        .runtime
        .bundled_conversation_module_as(
            "cwp://!cwr/actor_integrations/host",
            &requesting_actor_ref,
        )?
        .reserve_archive(&conversation)?;
    let control = ArchiveControl {
        reservation,
        writer: Arc::clone(writer),
        request_id,
        operation_id,
        command,
        requesting_actor_ref,
        conversation_ref,
        _activity: activity,
        _delivery: state.activity.delivery(),
    };
    if let Some(queue) = locked.conversation_controls.get_mut(&conversation) {
        queue.push_back(control);
        return Ok(());
    }
    locked
        .conversation_controls
        .insert(conversation.clone(), [control].into());
    let worker_state = Arc::clone(state);
    let worker_conversation = conversation.clone();
    if let Err(error) = thread::Builder::new()
        .name("cwr-host-archive".into())
        .spawn(move || drain_controls(&worker_state, &worker_conversation))
    {
        locked.conversation_controls.remove(&conversation);
        return Err(error.into());
    }
    Ok(())
}

fn drain_controls(state: &SharedState, conversation: &str) {
    loop {
        let control = {
            let Ok(mut locked) = lock_state(state) else {
                return;
            };
            let control = locked
                .conversation_controls
                .get_mut(conversation)
                .and_then(std::collections::VecDeque::pop_front);
            if control.is_none() {
                locked.conversation_controls.remove(conversation);
            }
            control
        };
        let Some(control) = control else {
            return;
        };
        if let Err(error) = execute_reserved(
            state,
            &control.writer,
            (control.request_id.clone(), control.operation_id),
            &control.command,
            &control.requesting_actor_ref,
            control.conversation_ref.as_deref(),
            Some(control.reservation),
        ) {
            let _ = write_event(
                &control.writer,
                &RuntimeServiceEvent::CommandFailed {
                    request_id: control.request_id,
                    error: error.to_string(),
                },
            );
        }
    }
}
