impl LocalRuntime {
    pub fn stop_selected_request_as(
        &self,
        conversation_ref: &str,
        invocation_ref: &str,
        requested_by_actor_ref: &str,
        request_id: &str,
        reason: Option<String>,
    ) -> CwrResult<LocalStopOutcome> {
        self.apply_stop(|state| {
            state.admission.plan_host_stop(
                conversation_ref,
                InvocationStopRequest {
                    requested_at_unix_ms: current_unix_ms(),
                    requested_by_actor_ref: Some(requested_by_actor_ref.into()),
                    request_id: Some(request_id.into()),
                    reason,
                },
                Some(invocation_ref),
                None,
            )
        })
    }

    /// Applies a host stop once, preserving its original target across retries.
    /// Provider timestamps select work present at the click, including work that
    /// completed before delivery, so a delayed event cannot stop a newer turn.
    pub fn stop_conversation_request_as(
        &self,
        conversation_ref: &str,
        requested_by_actor_ref: &str,
        request_id: &str,
        event_at_unix_ms: Option<u64>,
        reason: Option<String>,
    ) -> CwrResult<LocalStopOutcome> {
        self.apply_stop(|state| {
            let request = InvocationStopRequest {
                requested_at_unix_ms: current_unix_ms(),
                requested_by_actor_ref: Some(requested_by_actor_ref.into()),
                request_id: Some(request_id.into()),
                reason,
            };
            state
                .admission
                .plan_conversation_stop(conversation_ref, request, event_at_unix_ms)
        })
    }
}
