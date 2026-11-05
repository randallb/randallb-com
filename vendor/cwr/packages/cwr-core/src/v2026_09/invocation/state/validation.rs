impl ActionInvocationLedger {
    pub fn validate(&self) -> CwrResult<()> {
        self.validate_selected(None)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Ledger validation checks the persisted state invariants together, for all records at load or the changed records during a transaction."
    )]
    fn validate_selected(&self, selected: Option<&BTreeSet<String>>) -> CwrResult<()> {
        let mut origins = BTreeSet::new();
        let records: Box<dyn Iterator<Item = (&String, &ActionInvocationRecord)> + '_> =
            match selected {
                Some(refs) => Box::new(
                    refs.iter()
                        .filter_map(|key| self.invocations.get_key_value(key)),
                ),
                None => Box::new(self.invocations.iter()),
            };
        for (key, invocation) in records {
            if key != &invocation.invocation_ref {
                return invalid(format!(
                    "invocation ledger key {key} does not match {}",
                    invocation.invocation_ref
                ));
            }
            validate_ref("invocation ref", &invocation.invocation_ref)?;
            validate_ref("requester actor ref", &invocation.requester_actor_ref)?;
            validate_ref("target actor ref", &invocation.target_actor_ref)?;
            validate_optional_ref("conversation ref", invocation.conversation_ref.as_deref())?;
            validate_optional_ref(
                "actor integration ref",
                invocation.actor_integration_ref.as_deref(),
            )?;
            validate_optional_ref(
                "parent invocation ref",
                invocation.parent_invocation_ref.as_deref(),
            )?;
            validate_optional_ref(
                "source message ref",
                invocation.source_message_ref.as_deref(),
            )?;
            if invocation.source_message_ref.is_some() == invocation.timeline_publications.is_empty()
                && !(invocation.forked_from_invocation_ref.is_some()
                    && invocation.source_message_ref.is_none()
                    && !invocation.timeline_publications.is_empty())
            {
                return invalid(format!(
                    "invocation {key} must retain its source and timeline publications together"
                ));
            }
            for (publication_ref, disposition) in &invocation.timeline_publications {
                if invocation.conversation_ref.is_none() {
                    return invalid(format!(
                        "invocation {key} has a timeline publication without a conversation"
                    ));
                }
                validate_absolute_ref("timeline publication ref", publication_ref)?;
                if publication_ref == &invocation.invocation_ref
                    || invocation.source_message_ref.as_deref()
                        == Some(publication_ref.as_str())
                {
                    return invalid(format!(
                        "invocation {key} conflates invocation, source, or publication identity"
                    ));
                }
                if let crate::v2026_09::conversation::ConversationTimelineDisposition::Child {
                    parent_item_ref,
                } = disposition
                {
                    validate_absolute_ref("timeline parent item ref", parent_item_ref)?;
                }
            }
            if let Some(source) = &invocation.forked_from_invocation_ref {
                validate_ref("fork source invocation", source)?;
                if !invocation.state.is_terminal() || invocation.state == ActionInvocationState::Stopping || invocation.attempt_active || invocation.actor_integration_ref.is_some() {
                    return invalid("forked history cannot retain execution authority");
                }
            }
            validate_action_name(&invocation.action_name)?;
            match &invocation.origin {
                InvocationOrigin::Ingress {
                    integration_ref,
                    provider_event_ref,
                } => {
                    validate_ref("ingress integration ref", integration_ref)?;
                    validate_nonblank("provider event ref", provider_event_ref)?;
                }
                InvocationOrigin::HostCall {
                    caller_invocation_ref,
                    call_id,
                } => {
                    validate_ref("caller invocation ref", caller_invocation_ref)?;
                    validate_nonblank("host call id", call_id)?;
                }
            }
            validate_optional_ref(
                "stop-requesting actor ref",
                invocation.stop_requested_by_actor_ref.as_deref(),
            )?;
            if let Some(request_id) = &invocation.stop_request_id {
                validate_nonblank("stop request id", request_id)?;
            }
            if !origins.insert(invocation.origin.clone()) {
                return invalid(format!(
                    "invocation origin {:?} appears more than once",
                    invocation.origin
                ));
            }
            validate_record_parent(invocation, &self.invocations)?;
            validate_record_responses(invocation)?;
            let requires_actor_integration = matches!(
                invocation.state,
                ActionInvocationState::Running
                    | ActionInvocationState::Waiting
                    | ActionInvocationState::Completed
                    | ActionInvocationState::Blocked
                    | ActionInvocationState::Failed
            ) || (matches!(
                invocation.state,
                ActionInvocationState::Stopping | ActionInvocationState::Stopped
            ) && invocation_has_started(invocation));
            if requires_actor_integration && invocation.actor_integration_ref.is_none() && invocation.forked_from_invocation_ref.is_none() {
                return invalid(format!("started invocation {key} has no actor integration"));
            }
            if invocation
                .parent_invocation_ref
                .as_deref()
                .and_then(|parent_ref| self.invocations.get(parent_ref))
                .is_some_and(|parent| parent.state == ActionInvocationState::Stopped)
                && (!invocation.state.is_terminal()
                    || invocation.state == ActionInvocationState::Stopping)
            {
                return invalid(format!(
                    "attached invocation {key} outlived its stopped parent"
                ));
            }
            if invocation.stop_execution_pending
                && invocation.state != ActionInvocationState::Stopping
            {
                return invalid("only a stopping invocation may retain pending cancellation");
            }
            if invocation.attempt_active && invocation.state != ActionInvocationState::Running {
                return invalid(format!(
                    "only a running invocation may have an active attempt: {key}"
                ));
            }
            if invocation.attempt_count == 0 && invocation.started_at_unix_ms.is_some() {
                return invalid(format!(
                    "invocation {key} has a start time without an attempt"
                ));
            }
            if invocation.attempt_count > 0 && invocation.started_at_unix_ms.is_none() {
                return invalid(format!(
                    "invocation {key} has attempts without a start time"
                ));
            }
            if invocation.state == ActionInvocationState::Admitted && invocation.attempt_count != 0
            {
                return invalid(format!(
                    "admitted invocation {key} cannot have a started attempt"
                ));
            }
            if matches!(
                invocation.state,
                ActionInvocationState::Running
                    | ActionInvocationState::Waiting
                    | ActionInvocationState::Completed
                    | ActionInvocationState::Blocked
                    | ActionInvocationState::Failed
            ) && invocation.attempt_count == 0
            {
                return invalid(format!(
                    "{} invocation {key} must have a started attempt",
                    invocation_state_label(invocation.state)
                ));
            }
            if invocation
                .responses
                .iter()
                .any(|response| response.status.is_progress())
                && invocation.attempt_count == 0
            {
                return invalid(format!(
                    "invocation {key} has progress without a started attempt"
                ));
            }
            if (invocation.state.is_terminal()
                && invocation.state != ActionInvocationState::Stopping)
                != invocation.finished_at_unix_ms.is_some()
            {
                return invalid(format!(
                    "invocation {key} terminal state and finish time disagree"
                ));
            }
            if matches!(
                invocation.state,
                ActionInvocationState::Stopping | ActionInvocationState::Stopped
            ) && invocation.stop_requested_at_unix_ms.is_none()
            {
                return invalid(format!(
                    "stopped invocation {key} has no retained stop request"
                ));
            }
            if invocation.parent_invocation_ref.is_some() {
                let mut parent = invocation.parent_invocation_ref.as_deref();
                while let Some(parent_ref) = parent {
                    if parent_ref == invocation.invocation_ref {
                        return invalid(format!(
                            "attached invocation cycle contains {}",
                            invocation.invocation_ref
                        ));
                    }
                    parent = self
                        .invocations
                        .get(parent_ref)
                        .and_then(|parent| parent.parent_invocation_ref.as_deref());
                }
            }
        }

        let mut wait_calls = BTreeSet::new();
        let mut current_waiters = BTreeSet::new();
        for wait in &self.waits {
            validate_nonblank("await call id", &wait.call_id)?;
            if !wait_calls.insert((wait.waiter_invocation_ref.clone(), wait.call_id.clone())) {
                return invalid(format!(
                    "await call {} appears more than once for {}",
                    wait.call_id, wait.waiter_invocation_ref
                ));
            }
            let waiter = self
                .invocations
                .get(&wait.waiter_invocation_ref)
                .ok_or_else(|| invalid_error("waiter invocation is missing"))?;
            let awaited = self
                .invocations
                .get(&wait.awaited_invocation_ref)
                .ok_or_else(|| invalid_error("awaited invocation is missing"))?;
            if self.invocations.values().any(|invocation| {
                matches!(
                    &invocation.origin,
                    InvocationOrigin::HostCall {
                        caller_invocation_ref,
                        call_id,
                    } if caller_invocation_ref == &wait.waiter_invocation_ref
                        && call_id == &wait.call_id
                        && invocation.invocation_ref != wait.awaited_invocation_ref
                )
            }) {
                return invalid(format!(
                    "host call {} for {} is both a start and an await",
                    wait.call_id, wait.waiter_invocation_ref
                ));
            }
            if awaited.parent_invocation_ref.as_deref() != Some(wait.waiter_invocation_ref.as_str())
            {
                return invalid(format!(
                    "{} does not await a direct child",
                    wait.waiter_invocation_ref
                ));
            }
            if matches!(
                wait.state,
                InvocationWaitState::Pending
                    | InvocationWaitState::Ready
                    | InvocationWaitState::Resuming
            ) && !current_waiters.insert(wait.waiter_invocation_ref.clone())
            {
                return invalid(format!(
                    "{} has more than one current wait",
                    wait.waiter_invocation_ref
                ));
            }
            match wait.state {
                InvocationWaitState::Pending => {
                    if wait.response.is_some()
                        || waiter.state != ActionInvocationState::Waiting
                        || waiter.attempt_active
                        || awaited.state.is_terminal()
                    {
                        return invalid("pending invocation wait has inconsistent state");
                    }
                }
                InvocationWaitState::Ready => {
                    if wait.response.is_none()
                        || waiter.state != ActionInvocationState::Running
                        || waiter.attempt_active
                    {
                        return invalid("ready invocation wait has inconsistent state");
                    }
                }
                InvocationWaitState::Resuming => {
                    if wait.response.is_none()
                        || waiter.state != ActionInvocationState::Running
                        || !waiter.attempt_active
                    {
                        return invalid("resuming invocation wait has inconsistent state");
                    }
                }
                InvocationWaitState::Consumed => {
                    if wait.response.is_none() {
                        return invalid("consumed invocation wait has no response");
                    }
                }
                InvocationWaitState::Abandoned => {}
            }
            if let Some(response) = &wait.response {
                if response.invocation_ref != wait.awaited_invocation_ref
                    || !response.status.is_terminal()
                    || awaited.terminal_response() != Some(response)
                {
                    return invalid("invocation wait retains the wrong terminal response");
                }
            }
        }
        for invocation in self.invocations.values().filter(|invocation| {
            selected.is_none_or(|refs| refs.contains(&invocation.invocation_ref))
        }) {
            if invocation.state == ActionInvocationState::Waiting
                && !self.waits.iter().any(|wait| {
                    wait.waiter_invocation_ref == invocation.invocation_ref
                        && wait.state == InvocationWaitState::Pending
                })
            {
                return invalid(format!(
                    "waiting invocation {} has no pending wait",
                    invocation.invocation_ref
                ));
            }
        }
        Ok(())
    }
}
fn validate_start_request(request: &StartActionInvocation) -> CwrResult<()> {
    validate_ref("invocation ref", &request.invocation_ref)?;
    validate_ref("requester actor ref", &request.requester_actor_ref)?;
    validate_ref("target actor ref", &request.target_actor_ref)?;
    validate_optional_ref("conversation ref", request.conversation_ref.as_deref())?;
    validate_optional_ref(
        "actor integration ref",
        request.actor_integration_ref.as_deref(),
    )?;
    validate_optional_ref(
        "parent invocation ref",
        request.parent_invocation_ref.as_deref(),
    )?;
    validate_action_name(&request.action_name)?;
    match &request.origin {
        InvocationOrigin::Ingress {
            integration_ref,
            provider_event_ref,
        } => {
            validate_ref("ingress integration ref", integration_ref)?;
            validate_nonblank("provider event ref", provider_event_ref)?;
            if request.parent_invocation_ref.is_some() {
                return invalid("an ingress invocation cannot have a parent or attachment");
            }
        }
        InvocationOrigin::HostCall {
            caller_invocation_ref,
            call_id,
        } => {
            validate_ref("caller invocation ref", caller_invocation_ref)?;
            validate_nonblank("host call id", call_id)?;
            if request.parent_invocation_ref.as_deref() != Some(caller_invocation_ref) {
                return invalid("a host-call invocation parent must be its caller");
            }
        }
    }
    Ok(())
}

fn start_matches(existing: &ActionInvocationRecord, request: &StartActionInvocation) -> bool {
    existing.origin == request.origin
        && existing.requester_actor_ref == request.requester_actor_ref
        && existing.target_actor_ref == request.target_actor_ref
        && existing.conversation_ref == request.conversation_ref
        && existing.actor_integration_ref == request.actor_integration_ref
        && existing.action_name == request.action_name
        && existing.input == request.input
        && existing.parent_invocation_ref == request.parent_invocation_ref
}

fn validate_record_parent(
    invocation: &ActionInvocationRecord,
    invocations: &std::collections::BTreeMap<String, ActionInvocationRecord>,
) -> CwrResult<()> {
    match &invocation.origin {
        InvocationOrigin::Ingress { .. } => {
            if invocation.parent_invocation_ref.is_some() {
                return invalid("an ingress invocation cannot have a parent");
            }
        }
        InvocationOrigin::HostCall {
            caller_invocation_ref,
            call_id,
        } => {
            if invocation.parent_invocation_ref.as_deref() != Some(caller_invocation_ref) {
                return invalid("a host-call invocation parent must be its caller");
            }
            if !invocations.contains_key(caller_invocation_ref) {
                return invalid(format!("unknown parent invocation {caller_invocation_ref}"));
            }
            validate_nonblank("host call id", call_id)?;
        }
    }
    Ok(())
}

fn validate_record_responses(invocation: &ActionInvocationRecord) -> CwrResult<()> {
    let mut terminal_count = 0;
    for (index, response) in invocation.responses.iter().enumerate() {
        let expected_sequence = index as u64 + 1;
        if response.invocation_ref != invocation.invocation_ref
            || response.sequence != expected_sequence
        {
            return invalid(format!(
                "invocation {} has an invalid response sequence",
                invocation.invocation_ref
            ));
        }
        validate_nonblank("action response summary", &response.summary)?;
        if response.status.is_progress() {
            if response.result.is_some() || response.error.is_some() || response.cause.is_some() {
                return invalid("progress response contains terminal details");
            }
        } else if response.status.is_completed() {
            terminal_count += 1;
            if response.result.is_none() || response.error.is_some() || response.cause.is_some() {
                return invalid("completed response has invalid details");
            }
        } else if response.status.is_stopped() {
            terminal_count += 1;
            if response.result.is_some() || response.error.is_some() {
                return invalid("stopped response has invalid details");
            }
            if let Some(cause) = &response.cause {
                validate_detail(cause)?;
            }
        } else {
            debug_assert!(response.status.is_blocked() || response.status.is_failed());
            terminal_count += 1;
            if response.result.is_some() || response.error.is_none() || response.cause.is_some() {
                return invalid("error response has invalid details");
            }
            validate_detail(response.error.as_ref().unwrap())?;
        }
        if response.status.is_terminal() && index + 1 != invocation.responses.len() {
            return invalid("terminal action response is not last");
        }
    }
    if invocation.state.is_terminal() {
        if terminal_count != 1 {
            return invalid(format!(
                "terminal invocation {} has no single terminal response",
                invocation.invocation_ref
            ));
        }
        let status = invocation.responses.last().unwrap().status;
        if status.invocation_state() != Some(invocation.state)
            && !(invocation.state == ActionInvocationState::Stopping && status.is_stopped())
        {
            return invalid(format!(
                "invocation {} state and terminal response disagree",
                invocation.invocation_ref
            ));
        }
    } else if terminal_count != 0 {
        return invalid(format!(
            "nonterminal invocation {} has a terminal response",
            invocation.invocation_ref
        ));
    }
    if invocation.state == ActionInvocationState::Admitted && !invocation.responses.is_empty() {
        return invalid(format!(
            "admitted invocation {} has responses",
            invocation.invocation_ref
        ));
    }
    Ok(())
}

fn invocation_has_started(invocation: &ActionInvocationRecord) -> bool {
    invocation.attempt_count > 0
        || invocation.started_at_unix_ms.is_some()
        || invocation
            .responses
            .iter()
            .any(|response| response.status.is_progress())
}

fn invocation_state_label(state: ActionInvocationState) -> &'static str {
    match state {
        ActionInvocationState::AwaitingPerson => "awaiting_person",
        ActionInvocationState::Admitted => "admitted",
        ActionInvocationState::Running => "running",
        ActionInvocationState::Waiting => "waiting",
        ActionInvocationState::Completed => "completed",
        ActionInvocationState::Blocked => "blocked",
        ActionInvocationState::Failed => "failed",
        ActionInvocationState::Stopping => "stopping",
        ActionInvocationState::Stopped => "stopped",
    }
}

fn next_response_sequence(invocation: &ActionInvocationRecord) -> CwrResult<u64> {
    (invocation.responses.len() as u64)
        .checked_add(1)
        .ok_or_else(|| invalid_error("action response sequence overflow"))
}

fn validate_detail(detail: &ActionResponseDetail) -> CwrResult<()> {
    validate_nonblank("action response detail code", &detail.code)?;
    if let Some(message) = &detail.message {
        validate_nonblank("action response detail message", message)?;
    }
    Ok(())
}

fn validate_nonblank(label: &str, value: &str) -> CwrResult<()> {
    if value.trim().is_empty() {
        return invalid(format!("{label} must not be blank"));
    }
    Ok(())
}

fn validate_ref(label: &str, value: &str) -> CwrResult<()> {
    CwrRef::parse(value)
        .map(|_| ())
        .map_err(|error| invalid_error(format!("{label} is invalid: {error}")))
}

fn validate_absolute_ref(label: &str, value: &str) -> CwrResult<()> {
    let reference = CwrRef::parse(value)
        .map_err(|error| invalid_error(format!("{label} is invalid: {error}")))?;
    if value.starts_with("~@") || reference.fragment().is_some() {
        return invalid(format!("{label} must be absolute and unfragmented"));
    }
    Ok(())
}

fn validate_optional_ref(label: &str, value: Option<&str>) -> CwrResult<()> {
    if let Some(value) = value {
        validate_ref(label, value)?;
    }
    Ok(())
}

fn validate_action_name(value: &str) -> CwrResult<()> {
    if !is_action_name(value) {
        return invalid(format!(
            "action name must use the verb--object SVO grammar: {value}"
        ));
    }
    Ok(())
}

fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(invalid_error(message))
}

fn invalid_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}
