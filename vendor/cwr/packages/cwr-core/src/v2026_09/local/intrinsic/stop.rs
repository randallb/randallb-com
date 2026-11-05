impl LocalRuntime {
    fn stop_previous_intrinsic_invocation(
        &self,
        record: &ActionInvocationRecord,
        selector: Option<&str>,
    ) -> CwrResult<IntrinsicCwrResponse> {
        if selector.is_some() {
            return self.stop_intrinsic_invocation_with_request(
                record.conversation_ref.as_deref(),
                selector,
                &record.requester_actor_ref,
                Some("Stopped through @cwr".into()),
                Some(&record.invocation_ref),
            );
        }
        let previous = {
            let state = self.lock()?;
            let conversation_ref = record
                .conversation_ref
                .as_deref()
                .ok_or_else(|| invalid_error("CWR invocation has no conversation"))?;
            match state.admission.select_invocation_for_stop_excluding(
                Some(conversation_ref),
                None,
                Some(&record.invocation_ref),
            )? {
                InvocationStopSelection::Selected(invocation_ref) => Some(invocation_ref),
                InvocationStopSelection::None => None,
                InvocationStopSelection::Ambiguous(invocation_refs) => {
                    return Ok(intrinsic_cwr_invocation_stop_failed(
                        None,
                        format!(
                            "more than one invocation matches; use an exact ref: {}",
                            invocation_refs.join(", ")
                        ),
                    ));
                }
            }
        };
        self.stop_intrinsic_invocation_with_request(
            record.conversation_ref.as_deref(),
            previous.as_deref(),
            &record.requester_actor_ref,
            Some("Stopped through @cwr".into()),
            Some(&record.invocation_ref),
        )
    }
}
