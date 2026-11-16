impl ActionInvocationLedger {
    pub(crate) fn await_person(&mut self, reference: &str) -> CwrResult<()> {
        let record = self.invocations.get_mut(reference).ok_or_else(|| invalid_error("unknown person invocation"))?;
        if record.state != ActionInvocationState::Admitted { return invalid("person invocation already started"); }
        record.state = ActionInvocationState::AwaitingPerson;
        Ok(())
    }

    pub(crate) fn claim_person_reply(&mut self, reference: &str, now: u64) -> CwrResult<()> {
        let record = self.invocations.get_mut(reference).ok_or_else(|| invalid_error("unknown person invocation"))?;
        if record.state == ActionInvocationState::AwaitingPerson {
            record.state = ActionInvocationState::Admitted;
            self.claim(reference, now)?;
        }
        Ok(())
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.invocations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.invocations.is_empty()
    }

    pub fn invocation(&self, invocation_ref: &str) -> Option<&ActionInvocationRecord> {
        self.invocations.get(invocation_ref)
    }

    pub(crate) fn retain_reply_message_ref(
        &mut self,
        invocation_ref: &str,
        message_ref: &str,
    ) -> CwrResult<()> {
        let record = self.invocations.get_mut(invocation_ref)
            .ok_or_else(|| invalid_error("unknown invocation for reply publication"))?;
        if !record.state.is_terminal() {
            return invalid("reply publication requires a terminal invocation");
        }
        if record.reply_message_ref.as_deref().is_some_and(|existing| existing != message_ref) {
            return invalid("invocation already has a different published reply");
        }
        record.reply_message_ref = Some(message_ref.to_owned());
        Ok(())
    }

    pub fn invocations(&self) -> impl Iterator<Item = &ActionInvocationRecord> {
        self.invocations.values()
    }

    pub fn waits(&self) -> &[InvocationWait] {
        &self.waits
    }

    pub fn runnable_invocation_refs(&self) -> Vec<String> {
        self.invocations
            .values()
            .filter(|invocation| invocation.is_runnable())
            .map(|invocation| invocation.invocation_ref.clone())
            .collect()
    }

    pub fn start(&mut self, request: StartActionInvocation) -> CwrResult<StartInvocationOutcome> {
        validate_start_request(&request)?;
        if let Some(existing) = self
            .invocations
            .values()
            .find(|invocation| invocation.origin == request.origin)
        {
            if start_matches(existing, &request) {
                return Ok(StartInvocationOutcome::Existing(existing.clone()));
            }
            return invalid(format!(
                "invocation origin {:?} was replayed with a different request",
                request.origin
            ));
        }
        if self.invocations.contains_key(&request.invocation_ref) {
            return invalid(format!(
                "invocation ref {} is already in use",
                request.invocation_ref
            ));
        }
        if let InvocationOrigin::HostCall {
            caller_invocation_ref,
            call_id,
        } = &request.origin
        {
            if self.waits.iter().any(|wait| {
                wait.waiter_invocation_ref == *caller_invocation_ref && wait.call_id == *call_id
            }) {
                return invalid(format!(
                    "host call {call_id} for {caller_invocation_ref} is already an await"
                ));
            }
            let parent = self
                .invocations
                .get(caller_invocation_ref)
                .ok_or_else(|| invalid_error(format!("unknown parent {caller_invocation_ref}")))?;
            if parent.state != ActionInvocationState::Running || !parent.attempt_active {
                return invalid(format!(
                    "parent {caller_invocation_ref} is not in an active running attempt"
                ));
            }
        }

        let admission_sequence = self
            .invocations
            .values()
            .map(|record| record.admission_sequence)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| invalid_error("invocation admission sequence overflow"))?;
        let record = ActionInvocationRecord {
            forked_from_invocation_ref: None,
            invocation_ref: request.invocation_ref.clone(),
            origin: request.origin,
            requester_actor_ref: request.requester_actor_ref,
            target_actor_ref: request.target_actor_ref,
            conversation_ref: request.conversation_ref,
            actor_integration_ref: request.actor_integration_ref,
            action_name: request.action_name,
            input: request.input,
            parent_invocation_ref: request.parent_invocation_ref,
            source_message_ref: None,
            timeline_publications: BTreeMap::new(),
            state: ActionInvocationState::Admitted,
            responses: Vec::new(),
            reply_message_ref: None,
            admission_sequence,
            admitted_at_unix_ms: request.admitted_at_unix_ms,
            started_at_unix_ms: None,
            stop_requested_at_unix_ms: None,
            stop_requested_by_actor_ref: None,
            stop_request_id: None,
            stop_reason: None,
            stop_execution_pending: false,
            stop_settlement_tracked: false,
            finished_at_unix_ms: None,
            attempt_count: 0,
            attempt_started_at_unix_ms: None,
            attempt_started_after_activity_sequence: None,
            attempt_active: false,
        };
        self.invocations
            .insert(record.invocation_ref.clone(), record.clone());
        Ok(StartInvocationOutcome::Started(record))
    }

    /// Retains the causal source and one publication decision together.
    /// Replaying the exact decision is idempotent; later distinct publication
    /// refs append placements without changing earlier facts.
    pub fn retain_timeline_publication(
        &mut self,
        invocation_ref: &str,
        source_message_ref: &str,
        publication_ref: &str,
        disposition: crate::v2026_09::conversation::ConversationTimelineDisposition,
    ) -> CwrResult<()> {
        validate_ref("source message ref", source_message_ref)?;
        validate_absolute_ref("timeline publication ref", publication_ref)?;
        if publication_ref == source_message_ref || publication_ref == invocation_ref {
            return invalid(
                "invocation, source message, and timeline publication refs must be distinct",
            );
        }
        if let crate::v2026_09::conversation::ConversationTimelineDisposition::Child {
            parent_item_ref,
        } = &disposition
        {
            validate_absolute_ref("timeline parent item ref", parent_item_ref)?;
        }
        let invocation = self
            .invocations
            .get_mut(invocation_ref)
            .ok_or_else(|| invalid_error("unknown invocation for timeline publication"))?;
        if invocation.conversation_ref.is_none() {
            return invalid("an invocation outside a conversation cannot publish to a timeline");
        }
        if invocation
            .source_message_ref
            .as_deref()
            .is_some_and(|existing| existing != source_message_ref)
        {
            return invalid("invocation timeline publication was replayed with a different source");
        }
        if let Some(existing) = invocation.timeline_publications.get(publication_ref) {
            if existing != &disposition {
                return invalid(
                    "invocation timeline publication was replayed with a different disposition",
                );
            }
            invocation.source_message_ref = Some(source_message_ref.into());
            return Ok(());
        }
        invocation.source_message_ref = Some(source_message_ref.into());
        invocation
            .timeline_publications
            .insert(publication_ref.into(), disposition);
        Ok(())
    }

    pub fn claim(
        &mut self,
        invocation_ref: &str,
        started_at_unix_ms: u64,
    ) -> CwrResult<ClaimInvocationOutcome> {
        self.claim_after_activity(invocation_ref, started_at_unix_ms, None)
    }

    pub(crate) fn claim_after_activity(
        &mut self,
        invocation_ref: &str,
        started_at_unix_ms: u64,
        after_activity_sequence: Option<u64>,
    ) -> CwrResult<ClaimInvocationOutcome> {
        let invocation = self
            .invocations
            .get(invocation_ref)
            .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
        if !invocation.is_runnable() {
            return Ok(ClaimInvocationOutcome::NotRunnable {
                state: invocation.state,
                attempt_active: invocation.attempt_active,
            });
        }
        if invocation.actor_integration_ref.is_none() {
            return invalid(format!(
                "invocation {invocation_ref} has no selected actor integration"
            ));
        }
        let attempt_count = invocation
            .attempt_count
            .checked_add(1)
            .ok_or_else(|| invalid_error("invocation attempt count overflow"))?;
        let invocation = self.invocations.get_mut(invocation_ref).unwrap();
        invocation.state = ActionInvocationState::Running;
        invocation.attempt_active = true;
        invocation.attempt_count = attempt_count;
        invocation.attempt_started_at_unix_ms = Some(started_at_unix_ms);
        invocation.attempt_started_after_activity_sequence = after_activity_sequence;
        invocation
            .started_at_unix_ms
            .get_or_insert(started_at_unix_ms);
        Ok(ClaimInvocationOutcome::Claimed(Box::new(
            ActionInvocationClaim {
                invocation: invocation.clone(),
            },
        )))
    }

    /// Releases attempt claims that could have been left active by a process
    /// crash. The action remains `running`, preserving progress already emitted.
    pub fn recover_interrupted_attempts(&mut self) -> Vec<String> {
        let recovered = self
            .invocations
            .values()
            .filter(|invocation| invocation.attempt_active)
            .map(|invocation| invocation.invocation_ref.clone())
            .collect::<Vec<_>>();
        for invocation_ref in &recovered {
            self.release_interrupted_attempt(invocation_ref);
        }
        recovered
    }

    /// Releases one interrupted attempt without disturbing unrelated live attempts.
    pub fn recover_interrupted_attempt(&mut self, invocation_ref: &str) -> CwrResult<bool> {
        if !self.invocations.contains_key(invocation_ref) {
            return invalid(format!("unknown invocation ref {invocation_ref}"));
        }
        Ok(self.release_interrupted_attempt(invocation_ref))
    }

    fn release_interrupted_attempt(&mut self, invocation_ref: &str) -> bool {
        let Some(invocation) = self.invocations.get_mut(invocation_ref) else {
            return false;
        };
        if !invocation.attempt_active {
            return false;
        }
        invocation.attempt_active = false;
        true
    }

    pub fn record_progress(
        &mut self,
        invocation_ref: &str,
        status: ActionResponseStatus,
        summary: impl Into<String>,
    ) -> CwrResult<ActionResponse> {
        self.record_measured_progress(invocation_ref, status, summary, None)
    }

    pub fn record_measured_progress(
        &mut self, invocation_ref: &str, status: ActionResponseStatus,
        summary: impl Into<String>, progress: Option<crate::v2026_09::invocation::ActionProgress>,
    ) -> CwrResult<ActionResponse> {
        if let Some(progress) = &progress { progress.validate().map_err(invalid_error)?; }
        if !status.is_progress() {
            return invalid(format!(
                "action response status {} is not progress",
                status.as_u16()
            ));
        }
        let summary = summary.into();
        validate_nonblank("progress summary", &summary)?;
        let invocation = self.running_attempt(invocation_ref)?;
        let sequence = next_response_sequence(invocation)?;
        let response = ActionResponse {
            progress,
            invocation_ref: invocation_ref.into(),
            sequence,
            status,
            summary,
            result: None,
            error: None,
            cause: None,
        };
        self.invocations
            .get_mut(invocation_ref)
            .unwrap()
            .responses
            .push(response.clone());
        Ok(response)
    }

    pub fn finish(
        &mut self,
        invocation_ref: &str,
        terminal: InvocationTerminal,
        finished_at_unix_ms: u64,
    ) -> CwrResult<FinishInvocationOutcome> {
        let invocation = self
            .invocations
            .get(invocation_ref)
            .ok_or_else(|| invalid_error(format!("unknown invocation ref {invocation_ref}")))?;
        if invocation.state.is_terminal() {
            let response = invocation
                .terminal_response()
                .cloned()
                .ok_or_else(|| invalid_error("terminal invocation has no terminal response"))?;
            return Ok(FinishInvocationOutcome::AlreadyTerminal { response });
        }
        if invocation.state != ActionInvocationState::Running || !invocation.attempt_active {
            return invalid(format!(
                "invocation {invocation_ref} is not in an active running attempt"
            ));
        }
        let sequence = next_response_sequence(invocation)?;
        let InvocationTerminal {
            status,
            summary,
            result,
            error,
        } = terminal;
        if status.is_progress() {
            return invalid("progress must be recorded before terminalization");
        }
        if status.is_stopped() {
            return invalid("only the host stop path may create status 499");
        }
        let state = status
            .invocation_state()
            .ok_or_else(|| invalid_error("terminal response has no terminal state"))?;
        if status.is_completed() {
            if result.is_none() || error.is_some() {
                return invalid("completed response has invalid details");
            }
        } else {
            debug_assert!(status.is_blocked() || status.is_failed());
            if result.is_some() || error.is_none() {
                return invalid("error response has invalid details");
            }
        }
        validate_nonblank("terminal summary", &summary)?;
        if let Some(error) = &error {
            validate_detail(error)?;
        }
        let response = ActionResponse {
            progress: None,
            invocation_ref: invocation_ref.into(),
            sequence,
            status,
            summary,
            result,
            error,
            cause: None,
        };
        let invocation = self.invocations.get_mut(invocation_ref).unwrap();
        invocation.responses.push(response.clone());
        invocation.state = state;
        invocation.attempt_active = false;
        invocation.finished_at_unix_ms = Some(finished_at_unix_ms);
        Ok(FinishInvocationOutcome::Accepted {
            response,
            woken_invocation_refs: Vec::new(),
        })
    }

}
