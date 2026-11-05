impl ActionInvocationLedger {
    /// Confirms release of one canceled execution and settles its waiting ancestors.
    pub fn settle_stopped(
        &mut self,
        invocation_ref: &str,
        finished_at_unix_ms: u64,
    ) -> CwrResult<Vec<String>> {
        let invocation = self
            .invocations
            .get_mut(invocation_ref)
            .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
        if invocation.state != ActionInvocationState::Stopping || !invocation.stop_execution_pending
        {
            return Ok(Vec::new());
        }
        if finished_at_unix_ms < invocation.stop_requested_at_unix_ms.unwrap_or(0) {
            return invalid("stop settlement cannot precede its request");
        }
        invocation.stop_execution_pending = false;
        Ok(self.settle_stopping_tree(finished_at_unix_ms))
    }

    fn settle_stopping_tree(&mut self, now: u64) -> Vec<String> {
        let mut settled = Vec::new();
        loop {
            let mut pending_ancestors = BTreeSet::new();
            for pending in self.invocations.values().filter(|record| record.state == ActionInvocationState::Stopping) {
                let mut parent = pending.parent_invocation_ref.as_deref();
                let mut visited = BTreeSet::new();
                while let Some(reference) = parent {
                    if !visited.insert(reference) { break; }
                    pending_ancestors.insert(reference.to_owned());
                    parent = self.invocations.get(reference).and_then(|record| record.parent_invocation_ref.as_deref());
                }
            }
            let ready = self
                .invocations
                .values()
                .filter(|record| {
                    record.state == ActionInvocationState::Stopping
                        && !record.stop_execution_pending
                        && !pending_ancestors.contains(&record.invocation_ref)
                })
                .map(|record| record.invocation_ref.clone())
                .collect::<Vec<_>>();
            if ready.is_empty() {
                break;
            }
            for reference in ready {
                let record = self.invocations.get_mut(&reference).unwrap();
                settled.push(reference);
                record.state = ActionInvocationState::Stopped;
                record.finished_at_unix_ms =
                    Some(now.max(record.stop_requested_at_unix_ms.unwrap_or(0)));
            }
        }
        settled
    }

    pub fn stop(
        &mut self,
        invocation_ref: &str,
        request: InvocationStopRequest,
    ) -> CwrResult<StopInvocationOutcome> {
        let Some(invocation) = self.invocations.get(invocation_ref) else {
            return Ok(StopInvocationOutcome::Unknown);
        };
        if invocation.state.is_terminal() {
            if let Some(outcome) = self.retained_stop_outcome(invocation_ref, &request)? {
                return Ok(outcome);
            }
        }
        if let Some(requested_by_actor_ref) = &request.requested_by_actor_ref {
            validate_ref("stop-requesting actor ref", requested_by_actor_ref)?;
        }
        if let Some(request_id) = &request.request_id {
            validate_nonblank("stop request id", request_id)?;
        }
        if let Some(reason) = &request.reason {
            validate_nonblank("stop reason", reason)?;
        }

        let mut stopped_invocation_refs = Vec::new();
        self.collect_descendants_nonterminal(
            invocation_ref,
            &mut BTreeSet::new(),
            &mut stopped_invocation_refs,
        )?;
        for stopped_ref in &stopped_invocation_refs {
            let invocation = self.invocations.get(stopped_ref).unwrap();
            next_response_sequence(invocation)?;
        }
        let stopped_set = stopped_invocation_refs
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        for wait in &mut self.waits {
            if stopped_set.contains(&wait.waiter_invocation_ref)
                && matches!(
                    wait.state,
                    InvocationWaitState::Pending
                        | InvocationWaitState::Ready
                        | InvocationWaitState::Resuming
                )
            {
                wait.state = InvocationWaitState::Abandoned;
            }
        }

        let summary = request
            .reason
            .as_ref()
            .map(|reason| format!("Stopped: {reason}"))
            .unwrap_or_else(|| "Invocation stopped".into());
        let cause = request.reason.as_ref().map(|reason| {
            let mut cause = ActionResponseDetail::new("stop_requested").with_message(reason);
            if let Some(actor_ref) = &request.requested_by_actor_ref {
                cause = cause.with_details(json!({"requested_by_actor_ref": actor_ref}));
            }
            cause
        });
        let mut root_response = self.invocations.get(invocation_ref)
            .and_then(ActionInvocationRecord::terminal_response).cloned();
        for stopped_ref in &stopped_invocation_refs {
            let invocation = self.invocations.get_mut(stopped_ref).unwrap();
            let response = ActionResponse {
                progress: None,
                invocation_ref: stopped_ref.clone(),
                sequence: invocation.responses.len() as u64 + 1,
                status: ActionResponseStatus::STOPPED,
                summary: summary.clone(),
                result: None,
                error: None,
                cause: cause.clone(),
            };
            invocation.responses.push(response.clone());
            invocation.stop_execution_pending = invocation.attempt_active;
            invocation.stop_settlement_tracked = true;
            invocation.state = ActionInvocationState::Stopping;
            invocation.attempt_active = false;
            invocation.stop_requested_at_unix_ms = Some(request.requested_at_unix_ms);
            invocation.stop_requested_by_actor_ref = request.requested_by_actor_ref.clone();
            invocation.stop_request_id = request.request_id.clone();
            invocation.stop_reason = request.reason.clone();
            invocation.finished_at_unix_ms = None;
            if stopped_ref == invocation_ref {
                root_response = Some(response);
            }
        }
        self.settle_stopping_tree(request.requested_at_unix_ms);
        Ok(StopInvocationOutcome::Accepted {
            response: root_response.unwrap(),
            stopped_invocation_refs,
            woken_invocation_refs: Vec::new(),
            replayed: false,
        })
    }

    pub(crate) fn retained_stop_outcome(
        &self,
        invocation_ref: &str,
        request: &InvocationStopRequest,
    ) -> CwrResult<Option<StopInvocationOutcome>> {
        let Some(invocation) = self.invocations.get(invocation_ref) else {
            return Ok(Some(StopInvocationOutcome::Unknown));
        };
        if !invocation.state.is_terminal() {
            return Ok(None);
        }
        let original;
        let request = if invocation.state == ActionInvocationState::Stopping {
            original = InvocationStopRequest {
                requested_at_unix_ms: invocation.stop_requested_at_unix_ms.unwrap(),
                requested_by_actor_ref: invocation.stop_requested_by_actor_ref.clone(),
                request_id: invocation.stop_request_id.clone(),
                reason: invocation.stop_reason.clone(),
            };
            &original
        } else {
            request
        };
        let response = invocation
            .terminal_response()
            .cloned()
            .ok_or_else(|| invalid_error("terminal invocation has no terminal response"))?;
        if invocation.state != ActionInvocationState::Stopping
            && (request.request_id.is_none() || !stop_request_matches(invocation, request))
        {
            let mut running = Vec::new();
            self.collect_descendants_nonterminal(
                invocation_ref,
                &mut BTreeSet::new(),
                &mut running,
            )?;
            if !running.is_empty() {
                return Ok(None);
            }
            return Ok(Some(StopInvocationOutcome::TooLate { response }));
        }

        let mut stopped_invocation_refs = Vec::new();
        self.collect_descendants_matching_stop(
            invocation_ref,
            request,
            &mut BTreeSet::new(),
            &mut stopped_invocation_refs,
        )?;
        Ok(Some(StopInvocationOutcome::Accepted {
            response,
            stopped_invocation_refs,
            woken_invocation_refs: Vec::new(),
            replayed: true,
        }))
    }

    fn collect_descendants_nonterminal(
        &self,
        invocation_ref: &str,
        visited: &mut BTreeSet<String>,
        collected: &mut Vec<String>,
    ) -> CwrResult<()> {
        if !visited.insert(invocation_ref.into()) {
            return invalid(format!(
                "invocation parent cycle contains {invocation_ref}"
            ));
        }
        let invocation = self
            .invocations
            .get(invocation_ref)
            .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
        if !invocation.state.is_terminal() {
            collected.push(invocation_ref.into());
        }
        let children = self
            .invocations
            .values()
            .filter(|child| child.parent_invocation_ref.as_deref() == Some(invocation_ref))
            .map(|child| child.invocation_ref.clone())
            .collect::<Vec<_>>();
        for child in children {
            self.collect_descendants_nonterminal(&child, visited, collected)?;
        }
        Ok(())
    }

    fn collect_descendants_matching_stop(
        &self,
        invocation_ref: &str,
        request: &InvocationStopRequest,
        visited: &mut BTreeSet<String>,
        collected: &mut Vec<String>,
    ) -> CwrResult<()> {
        if !visited.insert(invocation_ref.into()) {
            return invalid(format!(
                "invocation parent cycle contains {invocation_ref}"
            ));
        }
        let invocation = self
            .invocations
            .get(invocation_ref)
            .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
        if stop_request_matches(invocation, request) {
            collected.push(invocation_ref.into());
        }
        let children = self
            .invocations
            .values()
            .filter(|child| child.parent_invocation_ref.as_deref() == Some(invocation_ref))
            .map(|child| child.invocation_ref.clone())
            .collect::<Vec<_>>();
        for child in children {
            self.collect_descendants_matching_stop(&child, request, visited, collected)?;
        }
        Ok(())
    }
}
fn stop_request_matches(
    invocation: &ActionInvocationRecord,
    request: &InvocationStopRequest,
) -> bool {
    matches!(
        invocation.state,
        ActionInvocationState::Stopping | ActionInvocationState::Stopped
    ) && invocation.stop_requested_at_unix_ms == Some(request.requested_at_unix_ms)
        && invocation.stop_requested_by_actor_ref == request.requested_by_actor_ref
        && invocation.stop_request_id == request.request_id
        && invocation.stop_reason == request.reason
}
