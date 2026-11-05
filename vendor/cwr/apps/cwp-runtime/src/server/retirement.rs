use super::*;

pub(super) fn attempt(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    nonce: String,
    expected_runtime_ref: String,
    expected_owner_pid: i32,
) -> CwrResult<()> {
    let publication = lock_broadcast(state)?;
    let mut locked = lock_state(state)?;
    if !locked.retirement_allowed
        || locked.controls_finalizing
        || locked.committed_retirement.is_some()
        || locked.generation_restart.is_some()
    {
        return invalid("configured retirement control is unavailable");
    }
    let receipt = owner_receipt(&locked, nonce, &expected_runtime_ref, expected_owner_pid)?;
    if locked
        .armed_retirement
        .as_ref()
        .is_some_and(|armed| *armed != receipt)
    {
        return invalid("another operation armed resident retirement");
    }
    let remaining = locked.runtime.control_drain("begin_generation")?;
    let commit = if remaining == 0 {
        locked.runtime.control_drain("restart").map(|_| ())
    } else {
        invalid("configured retirement is busy; no retirement committed")
    };
    if let Err(error) = commit {
        let cancel = locked.runtime.control_drain("cancel");
        drop(locked);
        drop(publication);
        cancel?;
        recover_pending(Arc::clone(state))?;
        return Err(error);
    }
    // Commit is irreversible. Save identity before any fallible acknowledgement;
    // a disconnected requester must recover its known operation, never resume.
    locked.committed_retirement = Some(receipt);
    // Keep communication adapters connected until main performs checked cleanup.
    // Closing their sockets here could turn planned retirement into an apparent
    // unexpected adapter exit while other host consumers use their grace period.
    locked
        .subscribers
        .retain(|subscriber| !Arc::ptr_eq(subscriber, writer));
    drop(locked);
    state.activity.retire_after_disconnect();
    let acknowledgement = command_succeeded(writer, request_id, None, Vec::new(), None);
    writer::close_after_output(writer, state.activity.delivery());
    acknowledgement
}

impl RuntimeServer {
    pub(crate) fn enable_configured_retirement(&self) -> CwrResult<()> {
        lock_state(&self.state)?.retirement_allowed = true;
        Ok(())
    }

    pub(crate) fn freeze_retirement_control(
        &self,
    ) -> CwrResult<Option<cwr::v2026_09::local::LocalResidentRetirement>> {
        let mut locked = lock_state(&self.state)?;
        Ok(freeze(&mut locked))
    }
}

fn owner_receipt(
    locked: &RuntimeServerState,
    nonce: String,
    expected_runtime_ref: &str,
    expected_owner_pid: i32,
) -> CwrResult<cwr::v2026_09::local::LocalResidentRetirement> {
    let receipt = cwr::v2026_09::local::LocalResidentRetirement {
        version: 1,
        nonce,
        runtime_ref: locked.runtime_ref.clone(),
        owner_pid: std::process::id() as i32,
        // SAFETY: geteuid only reads process identity and takes no pointers.
        owner_uid: unsafe { libc::geteuid() },
    };
    receipt.validate()?;
    if expected_runtime_ref != receipt.runtime_ref || expected_owner_pid != receipt.owner_pid {
        return invalid("configured retirement owner did not match");
    }
    Ok(receipt)
}

pub(super) fn arm(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    nonce: String,
    expected_runtime_ref: String,
    expected_owner_pid: i32,
) -> CwrResult<()> {
    let _publication = lock_broadcast(state)?;
    let mut locked = lock_state(state)?;
    if !locked.retirement_allowed
        || locked.controls_finalizing
        || locked.committed_retirement.is_some()
        || locked.generation_restart.is_some()
        || locked.runtime.has_drain_or_shutdown()?
    {
        return invalid("configured retirement arming is unavailable");
    }
    let receipt = owner_receipt(&locked, nonce, &expected_runtime_ref, expected_owner_pid)?;
    if locked
        .armed_retirement
        .as_ref()
        .is_some_and(|armed| *armed != receipt)
    {
        return invalid("another operation armed resident retirement");
    }
    // Requester disconnect or a lost ACK never forgets this known operation.
    locked.armed_retirement = Some(receipt);
    drop(locked);
    command_succeeded(writer, request_id, None, Vec::new(), None)
}

pub(super) fn freeze(
    locked: &mut RuntimeServerState,
) -> Option<cwr::v2026_09::local::LocalResidentRetirement> {
    if locked.controls_finalizing {
        return locked.committed_retirement.clone();
    }
    locked.controls_finalizing = true;
    if locked.committed_retirement.is_none() && locked.generation_restart.is_none() {
        if let Some(armed) = locked.armed_retirement.clone() {
            // Never retrofit success after managed shutdown cancels active work.
            // Fence+zero check+commit happen now, before shutdown, under control
            // ownership. A busy/other-drain/error path produces no receipt.
            if let Ok(remaining) = locked.runtime.control_drain("begin_generation") {
                if remaining == 0 && locked.runtime.control_drain("restart").is_ok() {
                    locked.committed_retirement = Some(armed);
                } else {
                    let _ = locked.runtime.control_drain("cancel");
                }
            }
        }
    }
    locked.committed_retirement.clone()
}
