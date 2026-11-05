//! Read-only negotiation with both current and older resident processes.
use super::{
    write_command, CwrResult, RuntimeServiceCommand, INTERNAL_BUILD_IDENTITY_REQUEST_ID,
    INTERNAL_FORK_PROBE_REQUEST_ID, INTERNAL_TRANSFER_PROBE_REQUEST_ID, RUNTIME_SERVICE_VERSION,
};
use std::{
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
};

pub(super) fn query_resident(writer: &Arc<Mutex<UnixStream>>) -> CwrResult<()> {
    // The legacy Drain envelope lets older residents reject unknown queries
    // without disconnecting. Identity must arrive before we project Ready.
    for (request_id, operation) in [
        (
            INTERNAL_BUILD_IDENTITY_REQUEST_ID,
            "supports-build-identity",
        ),
        (INTERNAL_FORK_PROBE_REQUEST_ID, "supports-conversation-fork"),
        (
            INTERNAL_TRANSFER_PROBE_REQUEST_ID,
            "supports-conversation-transfer",
        ),
    ] {
        write_command(
            writer,
            &RuntimeServiceCommand::Drain {
                version: RUNTIME_SERVICE_VERSION,
                request_id: request_id.into(),
                operation: operation.into(),
            },
        )?;
    }
    Ok(())
}
