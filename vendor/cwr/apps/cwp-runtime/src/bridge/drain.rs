use std::{
    io,
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{
        mpsc::{Receiver, TryRecvError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use cwr::{v2026_09::host::HostEvent, CwrResult};

const HOST_REQUEST_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const HOST_REQUEST_DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(super) fn pending_requests(
    event_reader: &Receiver<CwrResult<()>>,
    writer: &Arc<Mutex<UnixStream>>,
    output: &super::output::SharedHostWriter,
    pending: &super::pending::SharedPendingRequests,
    cancel_reads: bool,
) -> CwrResult<()> {
    if cancel_reads {
        cancel_abandoned_reads(writer, pending)?;
    }
    let deadline = Instant::now() + HOST_REQUEST_DRAIN_TIMEOUT;
    loop {
        match event_reader.try_recv() {
            Ok(outcome) => {
                fail_pending(
                    output,
                    pending,
                    "runtime connection closed before command completed",
                )?;
                return outcome;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                fail_pending(
                    output,
                    pending,
                    "runtime event reader stopped before command completed",
                )?;
                return Err(io::Error::other("runtime event reader exited unexpectedly").into());
            }
        }
        if super::pending::is_empty(pending)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(HOST_REQUEST_DRAIN_POLL_INTERVAL);
    }

    if let Ok(writer) = writer.lock() {
        let _ = writer.shutdown(Shutdown::Both);
    }
    fail_pending(
        output,
        pending,
        "host input closed before the runtime command completed",
    )?;
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "runtime commands did not complete before the host input drain deadline",
    )
    .into())
}

fn cancel_abandoned_reads(
    writer: &Arc<Mutex<UnixStream>>,
    pending: &super::pending::SharedPendingRequests,
) -> CwrResult<()> {
    for target_request_id in super::pending::disconnect_cancellations(pending)? {
        super::write_command(
            writer,
            &cwr::v2026_09::service::RuntimeServiceCommand::CancelRead {
                version: cwr::v2026_09::service::RUNTIME_SERVICE_VERSION,
                request_id: format!("host-disconnect-cancel:{target_request_id}"),
                target_request_id,
            },
        )?;
    }
    Ok(())
}

fn fail_pending(
    output: &super::output::SharedHostWriter,
    pending: &super::pending::SharedPendingRequests,
    error: &str,
) -> CwrResult<()> {
    let events = super::pending::take_unclaimed(pending)?
        .into_iter()
        .map(|request_id| HostEvent::CommandFailed {
            request_id,
            error: error.into(),
        })
        .collect::<Vec<_>>();
    if events.is_empty() {
        return Ok(());
    }
    super::output::write_events(output, &events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    fn host_disconnect_forwards_read_cancellation_before_draining() {
        let pending = super::super::pending::registry();
        super::super::pending::reserve_request(&pending, "read-page", None, false).unwrap();
        super::super::pending::mark_cancel_on_disconnect(&pending, "read-page").unwrap();
        let (runtime, bridge) = UnixStream::pair().unwrap();
        let writer = Arc::new(Mutex::new(bridge));

        cancel_abandoned_reads(&writer, &pending).unwrap();

        let line = BufReader::new(runtime).lines().next().unwrap().unwrap();
        let command: cwr::v2026_09::service::RuntimeServiceCommand =
            serde_json::from_str(&line).unwrap();
        assert!(matches!(
            command,
            cwr::v2026_09::service::RuntimeServiceCommand::CancelRead {
                target_request_id,
                ..
            } if target_request_id == "read-page"
        ));
    }
}
