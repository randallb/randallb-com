struct ProtocolMailbox {
    state: Mutex<ProtocolMailboxState>,
    ready: std::sync::Condvar,
}

struct ProtocolMailboxState {
    queue: VecDeque<io::Result<ProtocolDelivery>>,
    queued_bytes: usize,
    terminal: Option<io::Result<ProtocolDelivery>>,
    pending_skipped_progress: Option<(u64, u64)>,
    coalesce_progress: bool,
    closed: bool,
    cancelled: bool,
}

#[derive(Debug)]
enum ProtocolDelivery {
    Event {
        line: String,
        skipped_progress: Option<(u64, u64)>,
        progress_sequence: Option<u64>,
    },
    Terminal {
        line: String,
        skipped_progress: Option<(u64, u64)>,
    },
}

impl ProtocolMailbox {
    fn new() -> Self {
        Self {
            state: Mutex::new(ProtocolMailboxState {
                queue: VecDeque::new(),
                queued_bytes: 0,
                terminal: None,
                pending_skipped_progress: None,
                coalesce_progress: false,
                closed: false,
                cancelled: false,
            }),
            ready: std::sync::Condvar::new(),
        }
    }

    fn send(&self, received: io::Result<ProtocolDelivery>) -> bool {
        let received_bytes = delivery_bytes(&received);
        let mut state = self.state.lock().unwrap();
        while !can_enqueue(&state, received_bytes) && !state.cancelled {
            state = self.ready.wait(state).unwrap();
        }
        if state.cancelled {
            return false;
        }
        state.queued_bytes += received_bytes;
        state.queue.push_back(received);
        self.ready.notify_all();
        true
    }

    fn send_progress(&self, line: String, sequence: u64) -> bool {
        let line_bytes = line.len();
        let mut state = self.state.lock().unwrap();
        while !can_enqueue(&state, line_bytes)
            && !state.coalesce_progress
            && !state.cancelled
        {
            state = self.ready.wait(state).unwrap();
        }
        if state.cancelled {
            return false;
        }
        if !state.coalesce_progress {
            let skipped_progress = state.pending_skipped_progress.take();
            state.queued_bytes += line_bytes;
            state.queue.push_back(Ok(ProtocolDelivery::Event {
                line,
                skipped_progress,
                progress_sequence: Some(sequence),
            }));
            self.ready.notify_all();
            return true;
        }
        if let Some((_, last)) = state.pending_skipped_progress {
            if sequence != last.saturating_add(1) {
                return record_sequence_error(&mut state, &self.ready, last, sequence);
            }
            if !can_enqueue(&state, line_bytes) {
                state.pending_skipped_progress.as_mut().unwrap().1 = sequence;
                return true;
            }
            let skipped_progress = state.pending_skipped_progress.take();
            state.queued_bytes += line_bytes;
            state.queue.push_back(Ok(ProtocolDelivery::Event {
                line,
                skipped_progress,
                progress_sequence: Some(sequence),
            }));
            self.ready.notify_all();
            return true;
        }
        if !can_enqueue(&state, line_bytes) {
            if let Some(Ok(ProtocolDelivery::Event {
                line: previous_line,
                skipped_progress,
                progress_sequence: Some(previous),
                ..
            })) = state.queue.back()
            {
                let previous = *previous;
                if sequence != previous.saturating_add(1) {
                    return record_sequence_error(&mut state, &self.ready, previous, sequence);
                }
                let first = skipped_progress.map_or(previous, |(first, _)| first);
                let previous_bytes = previous_line.len();
                if state
                    .queued_bytes
                    .saturating_sub(previous_bytes)
                    .saturating_add(line_bytes)
                    > PROTOCOL_QUEUE_BYTES
                {
                    state.pending_skipped_progress = Some((sequence, sequence));
                    return true;
                }
                state.queue.pop_back();
                state.queued_bytes = state.queued_bytes - previous_bytes + line_bytes;
                state.queue.push_back(Ok(ProtocolDelivery::Event {
                    line,
                    skipped_progress: Some((first, previous)),
                    progress_sequence: Some(sequence),
                }));
                return true;
            }
            state.pending_skipped_progress = Some((sequence, sequence));
            return true;
        }
        state.queued_bytes += line_bytes;
        state.queue.push_back(Ok(ProtocolDelivery::Event {
            line,
            skipped_progress: None,
            progress_sequence: Some(sequence),
        }));
        self.ready.notify_all();
        true
    }

    fn send_terminal(&self, line: String) {
        let mut state = self.state.lock().unwrap();
        if state.cancelled {
            return;
        }
        let skipped_progress = state.pending_skipped_progress.take();
        state.terminal = Some(Ok(ProtocolDelivery::Terminal {
            line,
            skipped_progress,
        }));
        state.closed = true;
        self.ready.notify_all();
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        self.ready.notify_all();
    }

    fn cancel(&self) {
        let mut state = self.state.lock().unwrap();
        state.cancelled = true;
        self.ready.notify_all();
    }

    fn wait_for_terminal(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().unwrap();
        loop {
            if state.terminal.is_some() {
                return true;
            }
            if state.closed || state.cancelled {
                return false;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, wait) = self.ready.wait_timeout(state, remaining).unwrap();
            state = next;
            if wait.timed_out() && state.terminal.is_none() {
                return false;
            }
        }
    }

    fn set_progress_coalescing(&self, enabled: bool) {
        let mut state = self.state.lock().unwrap();
        state.coalesce_progress = enabled;
        self.ready.notify_all();
    }

    #[cfg(test)]
    fn try_recv(&self) -> Result<io::Result<ProtocolDelivery>, mpsc::TryRecvError> {
        let mut state = self.state.lock().unwrap();
        if let Some(received) = state.queue.pop_front() {
            state.queued_bytes = state.queued_bytes.saturating_sub(delivery_bytes(&received));
            self.ready.notify_all();
            return Ok(received);
        }
        if let Some(terminal) = state.terminal.take() {
            return Ok(terminal);
        }
        if state.closed {
            Err(mpsc::TryRecvError::Disconnected)
        } else {
            Err(mpsc::TryRecvError::Empty)
        }
    }

    fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<io::Result<ProtocolDelivery>, mpsc::RecvTimeoutError> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(received) = state.queue.pop_front() {
                state.queued_bytes = state.queued_bytes.saturating_sub(delivery_bytes(&received));
                self.ready.notify_all();
                return Ok(received);
            }
            if let Some(terminal) = state.terminal.take() {
                return Ok(terminal);
            }
            if state.closed {
                return Err(mpsc::RecvTimeoutError::Disconnected);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(mpsc::RecvTimeoutError::Timeout);
            }
            let (next, wait) = self.ready.wait_timeout(state, remaining).unwrap();
            state = next;
            if wait.timed_out()
                && state.queue.is_empty()
                && state.terminal.is_none()
                && !state.closed
            {
                return Err(mpsc::RecvTimeoutError::Timeout);
            }
        }
    }
}

fn delivery_bytes(delivery: &io::Result<ProtocolDelivery>) -> usize {
    match delivery {
        Ok(ProtocolDelivery::Event { line, .. } | ProtocolDelivery::Terminal { line, .. }) => {
            line.len()
        }
        Err(_) => 0,
    }
}

fn can_enqueue(state: &ProtocolMailboxState, bytes: usize) -> bool {
    state.queue.len() < PROTOCOL_QUEUE_FRAMES
        && state.queued_bytes.saturating_add(bytes) <= PROTOCOL_QUEUE_BYTES
}

fn record_sequence_error(
    state: &mut ProtocolMailboxState,
    ready: &std::sync::Condvar,
    previous: u64,
    actual: u64,
) -> bool {
    state.terminal = Some(Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "adapter event sequence must be {}, got {actual}",
            previous.saturating_add(1)
        ),
    )));
    state.closed = true;
    ready.notify_all();
    false
}
