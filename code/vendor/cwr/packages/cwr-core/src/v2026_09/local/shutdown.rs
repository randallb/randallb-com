impl LocalRuntime {
    /// Read-only control preflight; callers serialize lifecycle ownership around
    /// this observation and registration. This is not shutdown completion proof.
    pub fn has_drain_or_shutdown(&self) -> CwrResult<bool> {
        let state = self.lock()?;
        Ok(state.draining.is_some() || state.shutting_down)
    }

    /// Work that keeps a resident alive, even during an explicit drain.
    /// Persisted person requests hold no worker and survive idle shutdown.
    pub fn has_unfinished_work(&self) -> CwrResult<bool> {
        let state = self.lock()?;
        Ok(!state.active.is_empty()
            || state.admission.invocation_records().any(|record| {
                !record.state.is_terminal() && record.state != ActionInvocationState::AwaitingPerson
            }))
    }

    /// Fence new roots while allowing existing work and its children to finish.
    pub fn control_drain(&self, operation: &str) -> CwrResult<usize> {
        let mut state = self.lock()?;
        if state.shutting_down {
            return invalid("runtime shutdown has already begun");
        }
        match operation {
            "begin" | "begin_generation" => {
                if operation == "begin_generation" && state.draining.is_some() {
                    return invalid("another drain is already in progress");
                }
                if state.draining.is_none() {
                    state.draining = Some(
                        state
                            .admission
                            .invocation_records()
                            .filter(|record| {
                                !record.state.is_terminal() && record.started_at_unix_ms.is_some()
                            })
                            .map(|record| record.invocation_ref.clone())
                            .collect(),
                    );
                }
            }
            "cancel" => {
                state.draining = None;
                state.preserve_pending = false;
            }
            "status" | "finish" | "restart" => {}
            _ => return invalid("unknown runtime drain operation"),
        }
        let remaining = state
            .admission
            .invocation_records()
            .filter(|record| {
                !record.state.is_terminal() && drain_allows(&state, &record.invocation_ref)
            })
            .count()
            .max(state.active.len());
        if matches!(operation, "finish" | "restart") {
            if state.draining.is_none() || remaining != 0 {
                return invalid("runtime is not drained");
            }
            // Keep the fence recoverable until shutdown actually starts.
            state.preserve_pending = true;
            if operation == "restart" {
                // Commit the drained restart atomically with the last active-set check.
                // Unlike a reversible Desktop drain, this fence can no longer be cancelled.
                state.shutting_down = true;
            }
        }
        Ok(remaining)
    }

    /// Stops managed work and waits until every captured adapter process has
    /// been reaped. Calling shutdown also fences new execution claims.
    pub fn shutdown(&self, timeout: Duration) -> CwrResult<Vec<String>> {
        let (invocation_refs, controls) = {
            let mut state = self.lock()?;
            state.shutting_down = true;
            for pending in state.upstream_work.values_mut() {
                pending.disconnected = true;
            }
            let invocation_refs = state
                .admission
                .invocation_records()
                .filter(|record| {
                    !record.state.is_terminal()
                        && record.state != ActionInvocationState::AwaitingPerson
                        && !state.preserve_pending
                })
                .map(|record| record.invocation_ref.clone())
                .collect::<Vec<_>>();
            let controls = state.active.values().cloned().collect::<Vec<_>>();
            (invocation_refs, controls)
        };

        let mut stopped_invocation_refs = BTreeSet::new();
        let mut stop_errors = Vec::new();
        for invocation_ref in invocation_refs {
            if matches!(self.invocation_is_terminal(&invocation_ref), Ok(true)) {
                continue;
            }
            match self.stop_managed_invocation(&invocation_ref) {
                Ok(LocalStopOutcome::Accepted {
                    stopped_invocation_refs: stopped,
                    ..
                }) => stopped_invocation_refs.extend(stopped),
                Ok(LocalStopOutcome::TooLate { .. } | LocalStopOutcome::Unknown) => {}
                Err(error) => stop_errors.push(format!("{invocation_ref}: {error}")),
            }
        }
        let stop_result = if stop_errors.is_empty() {
            Ok(stopped_invocation_refs.into_iter().collect())
        } else {
            invalid(format!(
                "managed runtime could not stop every invocation: {}",
                stop_errors.join("; ")
            ))
        };

        // Stop controls are captured under the shutdown fence and signalled on
        // every path, including failed state publication.
        for control in controls {
            control.stop();
        }
        let drain_result = self.wait_for_active_execution(timeout);
        match (stop_result, drain_result) {
            (Ok(stopped), Ok(())) => Ok(stopped),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(stop_error), Err(drain_error)) => {
                Err(io::Error::other(format!(
                    "managed runtime shutdown failed: {stop_error}; active execution did not drain: {drain_error}"
                ))
                .into())
            }
        }
    }

    fn stop_managed_invocation(&self, invocation_ref: &str) -> CwrResult<LocalStopOutcome> {
        let request = InvocationStopRequest {
            requested_at_unix_ms: current_unix_ms(),
            requested_by_actor_ref: None,
            request_id: None,
            reason: Some("Managed runtime is shutting down".into()),
        };
        self.apply_stop(|_| Ok(Some((invocation_ref.into(), request))))
    }

    fn invocation_is_terminal(&self, invocation_ref: &str) -> CwrResult<bool> {
        Ok(self
            .lock()?
            .admission
            .invocation_record(invocation_ref)
            .is_none_or(|record| record.state.is_terminal()))
    }

    fn wait_for_active_execution(&self, timeout: Duration) -> CwrResult<()> {
        let started = Instant::now();
        loop {
            let active_invocation_refs = self.lock()?.active.keys().cloned().collect::<Vec<_>>();
            if active_invocation_refs.is_empty() {
                return Ok(());
            }
            let elapsed = started.elapsed();
            let Some(remaining) = timeout.checked_sub(elapsed) else {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "managed runtime shutdown timed out with active invocations: {}",
                        active_invocation_refs.join(", ")
                    ),
                )
                .into());
            };
            thread::sleep(remaining.min(Duration::from_millis(10)));
        }
    }
}

fn drain_allows(state: &LocalRuntimeState, invocation_ref: &str) -> bool {
    let Some(allowed) = &state.draining else {
        return true;
    };
    let mut current = invocation_ref;
    loop {
        if allowed.contains(current) {
            return true;
        }
        let Some(record) = state.admission.invocation_record(current) else {
            return false;
        };
        match &record.origin {
            InvocationOrigin::HostCall {
                caller_invocation_ref,
                ..
            } => current = caller_invocation_ref,
            InvocationOrigin::Ingress { .. } => return false,
        }
    }
}
