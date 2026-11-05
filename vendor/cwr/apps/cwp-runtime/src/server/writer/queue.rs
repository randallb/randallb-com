use std::collections::VecDeque;

use super::*;

pub(super) struct QueuedFrames {
    pub(super) frames: Vec<Vec<u8>>,
    coalescing_key: Option<CoalescingKey>,
    bytes: usize,
}

impl QueuedFrames {
    pub(super) fn new(
        frames: Vec<Vec<u8>>,
        coalescing_key: Option<CoalescingKey>,
    ) -> Result<Self, EnqueueFailure> {
        let bytes = frames.iter().try_fold(0usize, |total, frame| {
            total
                .checked_add(frame.len())
                .ok_or(EnqueueFailure::SizeOverflow)
        })?;
        Ok(Self {
            frames,
            coalescing_key,
            bytes,
        })
    }

    fn budgeted_bytes(&self) -> usize {
        if self.bytes > OUTPUT_QUEUE_NON_RESTART_BYTE_CAPACITY {
            0
        } else {
            self.bytes
        }
    }

    fn oversized(&self) -> usize {
        usize::from(self.bytes > OUTPUT_QUEUE_NON_RESTART_BYTE_CAPACITY)
    }
}

#[derive(Default)]
pub(super) struct OutputQueue {
    pub(super) pending: Mutex<PendingOutput>,
    available: Condvar,
}

#[derive(Default)]
pub(super) struct PendingOutput {
    batches: VecDeque<QueuedFrames>,
    // Reservations include the batch currently being written. It cannot be replaced.
    pub(super) budgeted_bytes: usize,
    oversized: usize,
    closed: bool,
}

impl OutputQueue {
    pub(super) fn push(&self, batch: QueuedFrames) -> Result<(), EnqueueFailure> {
        self.push_with_reserve(batch, false)
    }

    pub(super) fn push_restart(&self, batch: QueuedFrames) -> Result<(), EnqueueFailure> {
        self.push_with_reserve(batch, true)
    }

    fn push_with_reserve(
        &self,
        batch: QueuedFrames,
        use_restart_reserve: bool,
    ) -> Result<(), EnqueueFailure> {
        let mut pending = self.pending.lock().unwrap();
        if pending.closed {
            return Err(EnqueueFailure::Closing);
        }
        // Only obsolete, unsent upserts after the last barrier may be removed.
        // Appending their replacement preserves order of the surviving events.
        let replaced = batch.coalescing_key.as_ref().and_then(|coalescing_key| {
            pending
                .batches
                .iter()
                .enumerate()
                .rev()
                .take_while(|(_, batch)| batch.coalescing_key.is_some())
                .find(|(_, batch)| batch.coalescing_key.as_ref() == Some(coalescing_key))
                .map(|(index, _)| index)
        });
        let previous = replaced.map(|index| &pending.batches[index]);
        let bytes = pending.budgeted_bytes - previous.map_or(0, QueuedFrames::budgeted_bytes);
        let bytes = bytes
            .checked_add(batch.budgeted_bytes())
            .ok_or(EnqueueFailure::SizeOverflow)?;
        let oversized =
            pending.oversized - previous.map_or(0, QueuedFrames::oversized) + batch.oversized();
        let byte_capacity = OUTPUT_QUEUE_BYTE_CAPACITY
            - usize::from(!use_restart_reserve) * RESTART_OUTPUT_BYTE_RESERVE;
        if bytes > byte_capacity || oversized > 1 {
            return Err(EnqueueFailure::ByteBudget);
        }
        let batch_capacity = OUTPUT_QUEUE_CAPACITY - usize::from(!use_restart_reserve);
        if replaced.is_none() && pending.batches.len() >= batch_capacity {
            return Err(EnqueueFailure::QueueCount);
        }
        if let Some(index) = replaced {
            pending.batches.remove(index);
        }
        pending.budgeted_bytes = bytes;
        pending.oversized = oversized;
        pending.batches.push_back(batch);
        self.available.notify_all();
        Ok(())
    }

    // Indexed read replies are produced on independent worker threads. Unlike
    // live publication, those workers may wait for this client's output space.
    pub(super) fn push_read(
        &self,
        batch: QueuedFrames,
        deadline: std::time::Instant,
        cancelled: impl Fn() -> bool,
    ) -> io::Result<()> {
        debug_assert!(batch.coalescing_key.is_none());
        let mut pending = self.pending.lock().unwrap();
        loop {
            if pending.closed {
                return Err(disconnected());
            }
            if cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "conversation read operation was cancelled",
                ));
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "conversation read output exceeded its deadline",
                ));
            }
            let bytes = pending
                .budgeted_bytes
                .checked_add(batch.budgeted_bytes())
                .ok_or_else(|| EnqueueFailure::SizeOverflow.error())?;
            let oversized = pending.oversized + batch.oversized();
            if bytes <= OUTPUT_QUEUE_NON_RESTART_BYTE_CAPACITY
                && oversized <= 1
                && pending.batches.len() < OUTPUT_QUEUE_CAPACITY - 1
            {
                pending.budgeted_bytes = bytes;
                pending.oversized = oversized;
                pending.batches.push_back(batch);
                self.available.notify_all();
                return Ok(());
            }
            pending = self
                .available
                .wait_timeout(pending, remaining.min(Duration::from_millis(25)))
                .unwrap()
                .0;
        }
    }

    pub(super) fn next(&self) -> Option<QueuedFrames> {
        let mut pending = self.pending.lock().unwrap();
        loop {
            if let Some(batch) = pending.batches.pop_front() {
                return Some(batch);
            }
            if pending.closed {
                return None;
            }
            pending = self.available.wait(pending).unwrap();
        }
    }

    pub(super) fn complete(&self, batch: &QueuedFrames) {
        let mut pending = self.pending.lock().unwrap();
        pending.budgeted_bytes -= batch.budgeted_bytes();
        pending.oversized -= batch.oversized();
        self.available.notify_all();
    }

    pub(super) fn retire(&self) {
        let mut pending = self.pending.lock().unwrap();
        while let Some(batch) = pending.batches.pop_front() {
            pending.budgeted_bytes -= batch.budgeted_bytes();
            pending.oversized -= batch.oversized();
        }
        pending.closed = true;
        self.available.notify_all();
    }

    pub(super) fn close(&self) {
        self.pending.lock().unwrap().closed = true;
        self.available.notify_all();
    }
}

#[cfg(test)]
mod tests;
