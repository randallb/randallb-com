use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};

use cwr::{
    v2026_09::{
        conversation::ConversationReadCancellation, service::RuntimeServiceEvent,
        upstream::UpstreamEvent,
    },
    CwrResult,
};

const OUTPUT_QUEUE_CAPACITY: usize = 256;
pub(super) const OUTPUT_QUEUE_BYTE_CAPACITY: usize = 32 * 1024 * 1024;
const RESTART_OUTPUT_BYTE_RESERVE: usize = 16 * 1024;
const OUTPUT_QUEUE_NON_RESTART_BYTE_CAPACITY: usize =
    OUTPUT_QUEUE_BYTE_CAPACITY - RESTART_OUTPUT_BYTE_RESERVE;
const OUTPUT_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
mod queue;
mod reads;
use queue::{OutputQueue, QueuedFrames};
#[cfg(test)]
use reads::write_read_event;
pub(super) use reads::{deliver_read_result, read_reply_guard};

static NEXT_CLIENT_ID: AtomicUsize = AtomicUsize::new(1);
pub(super) type SharedWriter = Arc<RuntimeClientWriter>;

pub(super) struct RuntimeClientWriter {
    queue: Arc<OutputQueue>,
    state: Arc<OutputState>,
    read_reply: Mutex<()>,
    upstream_stop_sender: OnceLock<std::sync::mpsc::SyncSender<UpstreamEvent>>,
    #[cfg(test)]
    shutdown_stream: UnixStream,
}

struct OutputState {
    client_id: usize,
    connected: AtomicBool,
    metadata_bootstrap: AtomicBool,
    fork_metadata: AtomicBool,
    failure: OnceLock<&'static str>,
    restart_delivery: Mutex<Option<crate::idle::Delivery>>,
    reads: Mutex<ReadCancellationState>,
}

#[derive(Default)]
struct ReadCancellationState {
    active: BTreeMap<String, ConversationReadCancellation>,
    cancelled: BTreeSet<String>,
}

impl OutputState {
    fn cancel_reads(&self) {
        let mut state = self.reads.lock().unwrap();
        let reads = std::mem::take(&mut state.active);
        state.cancelled.clear();
        drop(state);
        for cancellation in reads.into_values() {
            cancellation.cancel();
        }
    }

    fn fail(&self, reason: &'static str, error: &io::Error) -> bool {
        if !self.connected.swap(false, Ordering::AcqRel) {
            return false;
        }
        let _ = self.failure.set(reason);
        self.cancel_reads();
        eprintln!(
            "cwp-runtime client output retired: client={} reason={reason} error={error}",
            self.client_id,
        );
        true
    }
}

#[derive(Clone, Copy, Debug)]
enum EnqueueFailure {
    QueueCount,
    ByteBudget,
    Closing,
    Disconnected,
    SizeOverflow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CoalescingKey {
    Conversation(String),
    TurnStatus(String),
}

impl EnqueueFailure {
    fn reason(self) -> &'static str {
        match self {
            Self::QueueCount => "queue_count",
            Self::ByteBudget => "byte_budget",
            Self::Closing => "writer_closing",
            Self::Disconnected => "writer_disconnected",
            Self::SizeOverflow => "size_overflow",
        }
    }

    fn error(self) -> io::Error {
        match self {
            Self::QueueCount => io::Error::new(io::ErrorKind::WouldBlock, format!("runtime client output queue is full ({OUTPUT_QUEUE_CAPACITY} batches)")),
            Self::ByteBudget => io::Error::new(io::ErrorKind::WouldBlock, format!("runtime client output byte budget is exhausted ({OUTPUT_QUEUE_BYTE_CAPACITY} bytes; one oversized batch allowed)")),
            Self::Closing => io::Error::new(
                io::ErrorKind::BrokenPipe,
                "runtime client writer is closing after queued output",
            ),
            Self::Disconnected => disconnected(),
            Self::SizeOverflow => io::Error::other("runtime client output size overflow"),
        }
    }
}

impl Drop for RuntimeClientWriter {
    fn drop(&mut self) {
        self.queue.close();
    }
}

pub(super) fn client_writer(stream: UnixStream) -> CwrResult<SharedWriter> {
    client_writer_with_timeout(stream, OUTPUT_WRITE_TIMEOUT)
}

fn client_writer_with_timeout(
    mut stream: UnixStream,
    write_timeout: Duration,
) -> CwrResult<SharedWriter> {
    stream.set_write_timeout(Some(write_timeout))?;
    #[cfg(test)]
    let shutdown_stream = stream.try_clone()?;
    let queue = Arc::new(OutputQueue::default());
    let output_queue = Arc::clone(&queue);
    let state = Arc::new(OutputState {
        client_id: NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed),
        connected: AtomicBool::new(true),
        metadata_bootstrap: AtomicBool::new(false),
        fork_metadata: AtomicBool::new(false),
        failure: OnceLock::new(),
        restart_delivery: Mutex::new(None),
        reads: Mutex::new(ReadCancellationState::default()),
    });
    let writer_state = Arc::clone(&state);
    thread::Builder::new()
        .name("cwp-runtime-client-writer".into())
        .spawn(move || {
            while let Some(batch) = output_queue.next() {
                let result = batch
                    .frames
                    .iter()
                    .try_for_each(|frame| stream.write_all(frame))
                    .and_then(|()| stream.flush());
                output_queue.complete(&batch);
                if let Err(error) = result {
                    writer_state.fail("socket_write", &error);
                    break;
                }
                if !writer_state.connected.load(Ordering::Acquire) {
                    break;
                }
            }
            output_queue.retire();
            writer_state.connected.store(false, Ordering::Release);
            writer_state.cancel_reads();
            let _ = stream.shutdown(Shutdown::Both);
            writer_state.restart_delivery.lock().unwrap().take();
        })?;
    Ok(Arc::new(RuntimeClientWriter {
        queue,
        state,
        read_reply: Mutex::new(()),
        upstream_stop_sender: OnceLock::new(),
        #[cfg(test)]
        shutdown_stream,
    }))
}

pub(super) fn upstream_stop_sender(
    writer: &SharedWriter,
) -> CwrResult<std::sync::mpsc::SyncSender<UpstreamEvent>> {
    if let Some(sender) = writer.upstream_stop_sender.get() {
        return Ok(sender.clone());
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel(64);
    let weak_writer = Arc::downgrade(writer);
    thread::Builder::new()
        .name("cwp-runtime-upstream-stop".into())
        .spawn(move || {
            while let Ok(event) = receiver.recv() {
                let Some(writer) = weak_writer.upgrade() else {
                    break;
                };
                let _ = write_event(&writer, &RuntimeServiceEvent::Upstream { event });
            }
        })?;
    if writer.upstream_stop_sender.set(sender.clone()).is_err() {
        return Ok(writer.upstream_stop_sender.get().unwrap().clone());
    }
    Ok(sender)
}

pub(super) fn write_event(writer: &SharedWriter, event: &RuntimeServiceEvent) -> CwrResult<()> {
    // Publication can hold the shared ordering lock. A client that falls behind
    // must reconnect and snapshot, never make other clients or execution wait.
    try_write_events(writer, std::slice::from_ref(event))
}

#[cfg(test)]
pub(super) fn try_write_event(writer: &SharedWriter, event: &RuntimeServiceEvent) -> CwrResult<()> {
    try_write_frames(writer, encode_events(std::slice::from_ref(event))?)
        .map_err(|failure| failure.error().into())
}

pub(super) fn try_write_events(
    writer: &SharedWriter,
    events: &[RuntimeServiceEvent],
) -> CwrResult<()> {
    try_write_frames(writer, encode_events(events)?).map_err(|failure| {
        let error = failure.error();
        if !matches!(failure, EnqueueFailure::Closing)
            && writer.state.fail(failure.reason(), &error)
        {
            writer.queue.retire();
        }
        error.into()
    })
}

pub(super) fn try_write_restart_events(
    writer: &SharedWriter,
    events: &[RuntimeServiceEvent],
) -> CwrResult<()> {
    try_write_frames_with(writer, encode_events(events)?, true).map_err(|failure| {
        let error = failure.error();
        if !matches!(failure, EnqueueFailure::Closing)
            && writer.state.fail(failure.reason(), &error)
        {
            writer.queue.retire();
        }
        error.into()
    })
}

#[cfg(test)]
pub(super) fn disconnect_writer(writer: &SharedWriter) {
    retire_writer(writer);
    let _ = writer.shutdown_stream.shutdown(Shutdown::Both);
}

pub(super) fn retire_writer(writer: &SharedWriter) {
    if writer.state.connected.swap(false, Ordering::AcqRel) {
        writer.state.cancel_reads();
        writer.queue.retire();
    }
}

pub(super) fn register_read_cancellation(
    writer: &SharedWriter,
    request_id: &str,
    cancellation: ConversationReadCancellation,
) {
    let mut reads = writer.state.reads.lock().unwrap();
    if reads.cancelled.remove(request_id) {
        cancellation.cancel();
    } else if writer.state.connected.load(Ordering::Acquire) {
        reads.active.insert(request_id.into(), cancellation);
    } else {
        cancellation.cancel();
    }
}

pub(super) fn complete_read(writer: &SharedWriter, request_id: &str) {
    let mut reads = writer.state.reads.lock().unwrap();
    reads.active.remove(request_id);
    reads.cancelled.remove(request_id);
}

pub(super) fn cancel_read(writer: &SharedWriter, request_id: &str) -> bool {
    let mut reads = writer.state.reads.lock().unwrap();
    let cancellation = reads.active.remove(request_id);
    reads.cancelled.insert(request_id.into());
    if let Some(cancellation) = cancellation {
        drop(reads);
        cancellation.cancel();
        true
    } else {
        false
    }
}

pub(super) fn read_was_cancelled(writer: &SharedWriter, request_id: &str) -> bool {
    writer
        .state
        .reads
        .lock()
        .unwrap()
        .cancelled
        .contains(request_id)
}

pub(super) fn writer_is_connected(writer: &SharedWriter) -> bool {
    writer.state.connected.load(Ordering::Acquire)
}

pub(super) fn enable_metadata_bootstrap(writer: &SharedWriter) {
    writer
        .state
        .metadata_bootstrap
        .store(true, Ordering::Release);
}

pub(super) fn uses_metadata_bootstrap(writer: &SharedWriter) -> bool {
    writer.state.metadata_bootstrap.load(Ordering::Acquire)
}

pub(super) fn enable_fork_metadata(writer: &SharedWriter) {
    writer.state.fork_metadata.store(true, Ordering::Release);
}

pub(super) fn includes_fork_metadata(writer: &SharedWriter) -> bool {
    writer.state.fork_metadata.load(Ordering::Acquire)
}

#[cfg(test)]
pub(super) fn client_writer_with_test_timeout(
    stream: UnixStream,
    write_timeout: Duration,
) -> CwrResult<SharedWriter> {
    client_writer_with_timeout(stream, write_timeout)
}

fn try_write_frames(writer: &SharedWriter, batch: QueuedFrames) -> Result<(), EnqueueFailure> {
    try_write_frames_with(writer, batch, false)
}

fn try_write_frames_with(
    writer: &SharedWriter,
    batch: QueuedFrames,
    restart: bool,
) -> Result<(), EnqueueFailure> {
    if !writer.state.connected.load(Ordering::Acquire) {
        return Err(EnqueueFailure::Disconnected);
    }
    if restart {
        writer.queue.push_restart(batch)
    } else {
        writer.queue.push(batch)
    }
}

fn encode_events(events: &[RuntimeServiceEvent]) -> CwrResult<QueuedFrames> {
    let coalescing_key = match events {
        [RuntimeServiceEvent::ConversationUpsert { conversation }] => Some(
            CoalescingKey::Conversation(conversation.conversation_ref.clone()),
        ),
        [RuntimeServiceEvent::ConversationTurnStatusSnapshot {
            conversation_ref, ..
        }] => Some(CoalescingKey::TurnStatus(conversation_ref.clone())),
        // Snapshots and every control/lifecycle event are ordering barriers.
        // Live output also contains required completed-commentary/clear pairs.
        _ => None,
    };
    let frames = events
        .iter()
        .map(|event| {
            let mut frame = serde_json::to_vec(event)?;
            frame.push(b'\n');
            Ok(frame)
        })
        .collect::<CwrResult<Vec<_>>>()?;
    QueuedFrames::new(frames, coalescing_key).map_err(|failure| failure.error().into())
}

fn disconnected() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "runtime client writer is disconnected",
    )
}

/// Deliver already-enqueued frames before closing a planned restart connection.
pub(super) fn close_after_output(writer: &SharedWriter, delivery: crate::idle::Delivery) {
    let mut pending = writer.state.restart_delivery.lock().unwrap();
    if writer.state.connected.load(Ordering::Acquire) {
        *pending = Some(delivery);
    }
    writer.queue.close();
}

#[cfg(test)]
mod tests;
