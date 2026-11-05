use super::*;
use std::{sync::MutexGuard, time::Instant};

// Admit one indexed read per client through materialization and output enqueue.
// Waiting workers retain only their request, rather than a large reply outside
// the queue's byte budget. Publication never acquires this permit.
pub(in crate::server) fn read_reply_guard<'a>(
    writer: &'a SharedWriter,
    request_id: &str,
    deadline: Instant,
) -> io::Result<MutexGuard<'a, ()>> {
    loop {
        if !writer_is_connected(writer) {
            return Err(disconnected());
        }
        if read_was_cancelled(writer, request_id) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "conversation read operation was cancelled",
            ));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "conversation read operation exceeded its deadline",
            ));
        }
        match writer.read_reply.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(5)),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(io::Error::other("read reply permit was poisoned"));
            }
        }
    }
}

pub(in crate::server) fn write_read_event(
    writer: &SharedWriter,
    event: &RuntimeServiceEvent,
    deadline: std::time::Instant,
) -> CwrResult<()> {
    let request_id = match event {
        RuntimeServiceEvent::RootConversationsPage { request_id, .. }
        | RuntimeServiceEvent::ConversationRecordsPage { request_id, .. }
        | RuntimeServiceEvent::CommandFailed { request_id, .. } => request_id,
        _ => return Err(io::Error::other("expected an indexed read result").into()),
    };
    let batch = encode_events(std::slice::from_ref(event))?;
    if let Err(error) = writer.queue.push_read(batch, deadline, || {
        !matches!(event, RuntimeServiceEvent::CommandFailed { .. })
            && read_was_cancelled(writer, request_id)
    }) {
        if matches!(
            error.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
        ) {
            // Prefer a small typed failure when it fits. If even that cannot
            // fit, the host's request timeout handles it without retiring the
            // connection or holding publication locks.
            let failure = RuntimeServiceEvent::CommandFailed {
                request_id: request_id.clone(),
                error: error.to_string(),
            };
            let _ = try_write_frames(writer, encode_events(&[failure])?);
        }
        return Err(error.into());
    }
    Ok(())
}

pub(in crate::server) fn deliver_read_result(
    writer: &SharedWriter,
    event: &RuntimeServiceEvent,
    deadline: Instant,
) {
    let request_id = match event {
        RuntimeServiceEvent::RootConversationsPage { request_id, .. }
        | RuntimeServiceEvent::ConversationRecordsPage { request_id, .. }
        | RuntimeServiceEvent::CommandFailed { request_id, .. } => request_id,
        _ => unreachable!("expected an indexed read result"),
    };
    if writer_is_connected(writer) {
        let _ = write_read_event(writer, event, deadline);
    }
    complete_read(writer, request_id);
}
