impl ActionInvocationLedger {
    /// Settles the suspended executions retained by versions with persisted awaits.
    /// Child outcomes and independently running descendants remain untouched.
    pub(crate) fn settle_legacy_suspended(
        &mut self,
        finished_at_unix_ms: u64,
    ) -> CwrResult<Vec<ActionResponse>> {
        let mut suspended = self.invocations.values()
            .filter(|record| record.state == ActionInvocationState::Waiting)
            .map(|record| record.invocation_ref.clone())
            .collect::<BTreeSet<_>>();
        for wait in &mut self.waits {
            if matches!(wait.state, InvocationWaitState::Pending | InvocationWaitState::Ready | InvocationWaitState::Resuming) {
                suspended.insert(wait.waiter_invocation_ref.clone());
                wait.state = InvocationWaitState::Abandoned;
            }
        }
        let mut responses = Vec::new();
        for invocation_ref in suspended {
            let record = self.invocations.get_mut(&invocation_ref)
                .ok_or_else(|| invalid_error("suspended invocation is unavailable"))?;
            if record.state.is_terminal() {
                continue;
            }
            let summary = "Execution interrupted by a runtime upgrade: suspended execution is no longer supported. Completed child results are retained.";
            let response = ActionResponse {
                progress: None,
                invocation_ref,
                sequence: next_response_sequence(record)?,
                status: ActionResponseStatus::FAILED,
                summary: summary.into(),
                result: None,
                error: Some(ActionResponseDetail::new("suspended_execution_unsupported").with_message(summary)),
                cause: None,
            };
            record.responses.push(response.clone());
            record.state = ActionInvocationState::Failed;
            record.attempt_active = false;
            record.finished_at_unix_ms = Some(finished_at_unix_ms);
            responses.push(response);
        }
        Ok(responses)
    }

    fn running_attempt(&self, invocation_ref: &str) -> CwrResult<&ActionInvocationRecord> {
        let invocation = self
            .invocations
            .get(invocation_ref)
            .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
        if invocation.state != ActionInvocationState::Running || !invocation.attempt_active {
            return invalid(format!(
                "invocation {invocation_ref} is not in an active running attempt"
            ));
        }
        Ok(invocation)
    }

}
