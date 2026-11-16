use super::*;

pub(super) fn control(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    operation: &str,
) -> CwrResult<()> {
    if matches!(
        operation,
        "supports-configured-retirement-v1" | "supports-configured-retirement-arm-v1"
    ) {
        let locked = lock_state(state)?;
        if !locked.retirement_allowed
            || locked.controls_finalizing
            || locked.committed_retirement.is_some()
        {
            return invalid("configured retirement is unavailable");
        }
        drop(locked);
        return write_event(
            writer,
            &RuntimeServiceEvent::DrainStatus {
                request_id,
                remaining: 0,
            },
        );
    }
    if operation == "supports-resident-observation-status" {
        let remaining = lock_state(state)?.runtime.control_drain("status")?;
        return write_event(
            writer,
            &RuntimeServiceEvent::DrainStatus {
                request_id,
                remaining,
            },
        );
    }
    if operation == "supports-build-identity" {
        return write_event(
            writer,
            &RuntimeServiceEvent::BuildIdentity {
                request_id,
                identity: crate::build_identity::current(),
            },
        );
    }
    if matches!(
        operation,
        "supports-generation-restart"
            | "supports-tui-graphql-ingress"
            | "supports-conversation-transfer"
            | "supports-conversation-fork"
    ) {
        // Legacy probes let older owners reject unsupported features without subscribing.
        return write_event(
            writer,
            &RuntimeServiceEvent::DrainStatus {
                request_id,
                remaining: 0,
            },
        );
    }
    if !matches!(operation, "begin" | "status" | "cancel" | "finish") {
        return invalid("unknown runtime drain operation");
    }
    let locked = lock_state(state)?;
    if locked.controls_finalizing
        || locked.committed_retirement.is_some()
        || locked.armed_retirement.is_some()
    {
        return invalid("resident retirement or finalization already owns control");
    }
    if locked.generation_restart.is_some() {
        return invalid("development runtime replacement already owns the drain");
    }
    let remaining = locked.runtime.control_drain(operation)?;
    drop(locked);
    write_event(
        writer,
        &RuntimeServiceEvent::DrainStatus {
            request_id,
            remaining,
        },
    )?;
    if operation == "cancel" {
        recover_pending(Arc::clone(state))?;
    }
    Ok(())
}
