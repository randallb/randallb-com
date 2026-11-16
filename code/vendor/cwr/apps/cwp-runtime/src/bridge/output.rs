use std::{
    io::{self, Write},
    os::unix::net::UnixStream,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use cwr::{v2026_09::host::HostEvent, CwrResult};

const HOST_OUTPUT_QUEUE_CAPACITY: usize = 8;
// A socket write may pause for this long without making progress. There is no
// additional wall-clock deadline for a queued batch or its acknowledgement.
const HOST_SOCKET_STALL_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) type SharedHostWriter = Arc<HostWriter>;

pub(crate) struct HostWriter {
    sender: Mutex<Option<SyncSender<HostOutputBatch>>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    abandoned: Arc<AtomicBool>,
}

struct HostOutputBatch {
    bytes: Vec<u8>,
    acknowledgement: SyncSender<io::Result<()>>,
}

pub(crate) fn host_writer() -> CwrResult<SharedHostWriter> {
    // Stdio belongs to the bridge process: its owner disconnects/kills that
    // process. Apply backpressure here, rather than abandoning a partial frame.
    host_writer_for(io::stdout())
}

pub(crate) fn host_socket_writer(stream: UnixStream) -> CwrResult<SharedHostWriter> {
    stream.set_write_timeout(Some(HOST_SOCKET_STALL_TIMEOUT))?;
    host_writer_for(stream)
}

// Both resident and TUI callers retain stderr in append-only logs. Keep
// per-event tracing out of this writer; Desktop records delivery separately.
pub(crate) fn host_writer_for(
    mut output: impl Write + Send + 'static,
) -> CwrResult<SharedHostWriter> {
    let (sender, receiver) = mpsc::sync_channel::<HostOutputBatch>(HOST_OUTPUT_QUEUE_CAPACITY);
    let abandoned = Arc::new(AtomicBool::new(false));
    let writer_abandoned = Arc::clone(&abandoned);
    let worker = thread::Builder::new()
        .name("cwp-runtime-host-output".into())
        .spawn(move || {
            while let Ok(batch) = receiver.recv() {
                let result = output.write_all(&batch.bytes).and_then(|()| output.flush());
                let failed = result.is_err();
                let _ = batch.acknowledgement.send(result);
                if failed || writer_abandoned.load(Ordering::Acquire) {
                    writer_abandoned.store(true, Ordering::Release);
                    break;
                }
            }
        })?;
    Ok(Arc::new(HostWriter {
        sender: Mutex::new(Some(sender)),
        worker: Mutex::new(Some(worker)),
        abandoned,
    }))
}

pub(crate) fn write_event(writer: &SharedHostWriter, event: &HostEvent) -> CwrResult<()> {
    write_events(writer, std::slice::from_ref(event))
}

pub(crate) fn write_events(writer: &SharedHostWriter, events: &[HostEvent]) -> CwrResult<()> {
    let mut bytes = Vec::new();
    for event in events {
        event.validate()?;
        serde_json::to_writer(&mut bytes, event)?;
        bytes.push(b'\n');
    }
    if writer.abandoned.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "host output writer is unavailable",
        )
        .into());
    }
    let (acknowledgement, completion) = mpsc::sync_channel(1);
    writer
        .sender
        .lock()
        .unwrap()
        .as_ref()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "host output writer is closed"))?
        .try_send(HostOutputBatch {
            bytes,
            acknowledgement,
        })
        .map_err(|error| match error {
            TrySendError::Full(_) => {
                io::Error::new(io::ErrorKind::WouldBlock, "host output queue is full")
            }
            TrySendError::Disconnected(_) => io::Error::new(
                io::ErrorKind::BrokenPipe,
                "host output writer is disconnected",
            ),
        })?;
    // Only the writer can acknowledge completion. In particular, queueing
    // behind an already-blocked write must not start a second, shorter timer.
    let outcome = completion.recv().map_err(|_| {
        writer.abandoned.store(true, Ordering::Release);
        io::Error::new(io::ErrorKind::BrokenPipe, "host output writer exited")
    })?;
    if outcome.is_err() {
        writer.abandoned.store(true, Ordering::Release);
    }
    outcome?;
    Ok(())
}

// Stop admitting output and wait for every admitted batch before the host
// listener shuts down the socket. A runtime/input error can otherwise race a
// host response that is still being written on the output thread.
pub(crate) fn finish(writer: &SharedHostWriter) -> CwrResult<()> {
    writer.sender.lock().unwrap().take();
    if let Some(worker) = writer.worker.lock().unwrap().take() {
        worker
            .join()
            .map_err(|_| io::Error::other("host output writer panicked"))?;
    }
    if writer.abandoned.load(Ordering::Acquire) {
        return Err(io::Error::new(io::ErrorKind::BrokenPipe, "host output writer failed").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
