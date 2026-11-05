impl ConversationAdmissionStore {
    /// Keeps a repeated host stop attached to its original invocation. Hosts
    /// supply the requesting actor and provider event time, not a guessed target.
    pub fn plan_conversation_stop(
        &self,
        conversation_ref: &str,
        request: InvocationStopRequest,
        event_at_unix_ms: Option<u64>,
    ) -> CwrResult<Option<(String, InvocationStopRequest)>> {
        self.plan_host_stop(conversation_ref, request, None, event_at_unix_ms)
    }

    /// An explicit selection must belong to this conversation and cannot be
    /// changed by a repeated request ID.
    pub fn plan_host_stop(
        &self,
        conversation_ref: &str,
        mut request: InvocationStopRequest,
        selected_invocation_ref: Option<&str>,
        event_at_unix_ms: Option<u64>,
    ) -> CwrResult<Option<(String, InvocationStopRequest)>> {
        let request_id = request
            .request_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| invalid_error("stop request ID must not be blank"))?;
        let actor_ref = request
            .requested_by_actor_ref
            .as_deref()
            .ok_or_else(|| invalid_error("conversation stop requires a requesting actor"))?;
        let previous = self.invocation_records().find(|record| {
            record.parent_invocation_ref.is_none()
                && matches!(record.origin, InvocationOrigin::Ingress { .. })
                && record.stop_request_id.as_deref() == Some(request_id)
        });
        let invocation_ref = if let Some(previous) = previous {
            if previous.conversation_ref.as_deref() != Some(conversation_ref)
                || previous.stop_requested_by_actor_ref.as_deref() != Some(actor_ref)
                || previous.stop_reason != request.reason
                || selected_invocation_ref
                    .is_some_and(|selected| selected != previous.invocation_ref)
            {
                return invalid("stop request ID was reused with different parameters");
            }
            request.requested_at_unix_ms = previous
                .stop_requested_at_unix_ms
                .ok_or_else(|| invalid_error("retained stop has no request time"))?;
            previous.invocation_ref.clone()
        } else if let Some(selected) = selected_invocation_ref {
            let record = self
                .invocation_record(selected)
                .ok_or_else(|| invalid_error("selected invocation was not found"))?;
            if record.conversation_ref.as_deref() != Some(conversation_ref)
                || record.parent_invocation_ref.is_some()
            {
                return invalid("selected invocation is not a root turn in this conversation");
            }
            selected.to_owned()
        } else {
            let selection = match event_at_unix_ms {
                Some(at) => self.select_invocation_for_stop_at(conversation_ref, at)?,
                None => self.select_invocation_for_stop(Some(conversation_ref), None)?,
            };
            match selection {
                InvocationStopSelection::Selected(reference) => reference,
                InvocationStopSelection::None => return Ok(None),
                InvocationStopSelection::Ambiguous(references) => {
                    return invalid(format!(
                        "more than one invocation can be stopped; use an exact ref: {}",
                        references.join(", ")
                    ));
                }
            }
        };
        if !self.is_actor_authorized_to_stop_invocation(&invocation_ref, actor_ref)? {
            return invalid("you may only stop work you requested or work addressed to you");
        }
        Ok(Some((invocation_ref, request)))
    }
}
