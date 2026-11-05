impl ConversationAdmissionStore {
    pub fn stop_invocation(
        &mut self,
        invocation_ref: &str,
        request: InvocationStopRequest,
    ) -> CwrResult<StopInvocationOutcome> {
        if let Some(outcome) = self
            .state
            .invocation_ledger
            .retained_stop_outcome(invocation_ref, &request)?
        {
            return Ok(outcome);
        }
        let invocation_ref = invocation_ref.to_owned();
        let occurred_at_unix_ms = request.requested_at_unix_ms;
        self.transact_invocations(move |state| {
            let outcome = state.invocation_ledger.stop(&invocation_ref, request)?;
            if let StopInvocationOutcome::Accepted {
                stopped_invocation_refs,
                replayed: false,
                ..
            } = &outcome
            {
                let stopped = stopped_invocation_refs
                    .iter()
                    .map(|stopped_ref| {
                        let response = state
                            .invocation_ledger
                            .invocation(stopped_ref)
                            .and_then(ActionInvocationRecord::terminal_response)
                            .cloned()
                            .ok_or_else(|| {
                                invalid_error(format!(
                                    "stopped invocation {stopped_ref} has no terminal response"
                                ))
                            })?;
                        Ok((stopped_ref.clone(), response))
                    })
                    .collect::<CwrResult<Vec<_>>>()?;
                for (stopped_ref, response) in stopped {
                    stage_raw_terminal_projection(
                        state,
                        &stopped_ref,
                        &response,
                        occurred_at_unix_ms,
                    )?;
                }
            }
            Ok(outcome)
        })
    }

    pub fn settle_stopped_invocation(
        &mut self,
        invocation_ref: &str,
        finished_at_unix_ms: u64,
    ) -> CwrResult<Vec<String>> {
        let invocation_ref = invocation_ref.to_owned();
        self.transact_invocations(move |state| {
            state
                .invocation_ledger
                .settle_stopped(&invocation_ref, finished_at_unix_ms)
        })
    }

    /// Persists the terminal response and all waiter wakeups atomically.
    pub fn finish_invocation(
        &mut self,
        invocation_ref: &str,
        terminal: InvocationTerminal,
        finished_at_unix_ms: u64,
    ) -> CwrResult<FinishInvocationOutcome> {
        let invocation_ref = invocation_ref.to_owned();
        self.transact_invocations(move |state| {
            let outcome =
                state
                    .invocation_ledger
                    .finish(&invocation_ref, terminal, finished_at_unix_ms)?;
            if let FinishInvocationOutcome::Accepted { response, .. } = &outcome {
                stage_raw_terminal_projection(
                    state,
                    &invocation_ref,
                    response,
                    finished_at_unix_ms,
                )?;
            }
            Ok(outcome)
        })
    }

    /// Releases attempt leases left active by a prior process. Opening a store
    /// is deliberately read-only; hosts call this once from their recovery
    /// path and then schedule the returned runnable invocation refs.
    pub fn recover_interrupted_invocations(&mut self) -> CwrResult<Vec<String>> {
        if !self
            .state
            .invocation_ledger
            .invocations()
            .any(|record| record.attempt_active)
        {
            return Ok(Vec::new());
        }
        self.transact(|staged| {
            let interrupted = staged
                .invocation_ledger
                .invocations()
                .filter(|record| record.attempt_active)
                .map(|record| record.invocation_ref.clone())
                .collect::<Vec<_>>();
            if interrupted.is_empty() {
                return Ok(Vec::new());
            }
            for invocation_ref in &interrupted {
                super::terminal::settle_interrupted_execution(staged, invocation_ref)?;
            }
            let recovered = staged.invocation_ledger.recover_interrupted_attempts();

            Ok(recovered)
        })
    }

    /// Releases one interrupted attempt while preserving every unrelated live
    /// attempt. Hosts use this after losing the in-process owner for one claim.
    pub fn recover_interrupted_invocation(&mut self, invocation_ref: &str) -> CwrResult<bool> {
        self.transact(|staged| {
            let active = staged
                .invocation_ledger
                .invocation(invocation_ref)
                .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?
                .attempt_active;
            if active && super::terminal::settle_interrupted_execution(staged, invocation_ref)? {
                return Ok(false);
            }
            let recovered = staged
                .invocation_ledger
                .recover_interrupted_attempt(invocation_ref)?;
            if !recovered {
                return Ok(false);
            }

            Ok(true)
        })
    }

    fn transact_invocations<T>(
        &mut self,
        mutate: impl FnOnce(&mut ConversationRuntimeState) -> CwrResult<T>,
    ) -> CwrResult<T> {
        self.transact(mutate)
    }
}
fn stage_raw_terminal_projection(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    response: &ActionResponse,
    occurred_at_unix_ms: u64,
) -> CwrResult<()> {
    let origin = state
        .invocation_ledger
        .invocation(invocation_ref)
        .map(|invocation| invocation.origin.clone())
        .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
    push_execution_activity_in_state(
        state,
        invocation_ref,
        super::terminal::terminal_activity(response)?,
        occurred_at_unix_ms,
    )?;
    if matches!(origin, InvocationOrigin::Ingress { .. }) {
        let admission = state
            .admissions
            .find_mut(|record| record.invocation_ref.as_deref() == Some(invocation_ref))
            .ok_or_else(|| {
                invalid_error(format!(
                    "root invocation {invocation_ref} has no matching admission"
                ))
            })?;
        if admission.state != ConversationAdmissionState::Admitted {
            return invalid(format!(
                "root invocation {invocation_ref} is not admitted for terminal projection"
            ));
        }
        admission.state = super::terminal::admission_state(response)?;
    }
    Ok(())
}
