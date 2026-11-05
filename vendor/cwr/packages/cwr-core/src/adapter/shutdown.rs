#[derive(Debug)]
struct ProcessGroupShutdown {
    grace: Duration,
    requested_at: Option<Instant>,
    stopped: bool,
}

impl ProcessGroupShutdown {
    fn new(grace: Duration) -> Self {
        Self {
            grace,
            requested_at: None,
            stopped: false,
        }
    }
}

impl ProcessAdapterStopControl {
    fn request_process_group_stop(&self, group: i32) -> io::Result<()> {
        let mut groups = self.lock_process_groups();
        let Some(state) = groups.get_mut(&group) else {
            return Ok(());
        };
        if state.stopped || state.requested_at.is_some() {
            return Ok(());
        }
        if state.grace.is_zero() {
            stop_adapter_process_group(group)?;
            state.stopped = true;
            return Ok(());
        }
        signal_adapter_process_group(group, libc::SIGTERM)?;
        let requested_at = Instant::now();
        state.requested_at = Some(requested_at);
        let grace = state.grace;
        let control = self.clone();
        // Escalate even if the host is blocked inside a host-call callback.
        // Removal on reap and the timestamp check prevent signalling a reused pid.
        thread::spawn(move || {
            thread::sleep(grace);
            let mut groups = control.lock_process_groups();
            if let Some(state) = groups.get_mut(&group) {
                if state.requested_at == Some(requested_at)
                    && !state.stopped
                    && stop_adapter_process_group(group).is_ok()
                {
                    state.stopped = true;
                }
            }
        });
        Ok(())
    }
}
