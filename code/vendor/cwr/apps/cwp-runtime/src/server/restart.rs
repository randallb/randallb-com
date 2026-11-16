use super::*;

pub(super) fn control(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    operation: &str,
    generation: String,
) -> CwrResult<()> {
    if generation.trim().is_empty() {
        return invalid("development runtime generation is empty");
    }
    let _publication = lock_broadcast(state)?;
    let mut locked = lock_state(state)?;
    if locked.controls_finalizing
        || locked.committed_retirement.is_some()
        || locked.armed_retirement.is_some()
    {
        return invalid("resident retirement or finalization already owns control");
    }
    match &locked.generation_restart {
        Some((owner, selected)) if Arc::ptr_eq(owner, writer) && *selected == generation => {}
        Some(_) => return invalid("another connection owns development runtime replacement"),
        None if operation == "begin" => {
            locked.runtime.control_drain("begin_generation")?;
            locked.generation_restart = Some((Arc::clone(writer), generation.clone()));
        }
        None => return invalid("development runtime replacement has not begun"),
    }
    let remaining = locked.runtime.control_drain(match operation {
        "begin" | "status" => "status",
        "cancel" => "cancel",
        "finish" => "restart",
        _ => return invalid("unknown development runtime replacement operation"),
    })?;
    if operation == "cancel" {
        locked.generation_restart = None;
    }
    drop(locked);
    let acknowledgement = RuntimeServiceEvent::DrainStatus {
        request_id,
        remaining,
    };
    if operation == "finish" {
        state.activity.restart_after_disconnect(&generation);
        let notice = RuntimeServiceEvent::RuntimeRestarting { generation };
        // A peer's disconnect broadcasts catalog changes. Detach these writers
        // under the publication lock so later broadcasts cannot retire a queue
        // that is still flushing its restart acknowledgement and notice.
        let subscribers = std::mem::take(&mut lock_state(state)?.subscribers);
        // The owner's acknowledgement and notice are one batch so committing a
        // restart needs only one queue slot and cannot accept one without the other.
        let result = writer::try_write_restart_events(writer, &[acknowledgement, notice.clone()]);
        for subscriber in subscribers
            .iter()
            .filter(|subscriber| !Arc::ptr_eq(subscriber, writer))
        {
            let _ = writer::try_write_restart_events(subscriber, std::slice::from_ref(&notice));
        }
        for subscriber in subscribers {
            writer::close_after_output(&subscriber, state.activity.delivery());
        }
        return result;
    }
    write_event(writer, &acknowledgement)
}

pub(super) fn cancel_disconnected(state: &SharedState, writer: &SharedWriter) {
    if let Ok(mut locked) = lock_state(state) {
        if locked
            .generation_restart
            .as_ref()
            .is_some_and(|(owner, _)| Arc::ptr_eq(owner, writer))
        {
            // Before commit, a failed/killed requester releases the fence. After
            // commit, LocalRuntime rejects cancellation and preserves queued work.
            if locked.runtime.control_drain("cancel").is_ok() {
                locked.generation_restart = None;
            }
        }
    }
}
