impl LocalRuntime {
    fn record_progress_activity(
        &self,
        invocation_ref: &str,
        status: u16,
        summary: String,
        published: bool,
        progress: Option<crate::v2026_09::invocation::ActionProgress>,
    ) -> CwrResult<ConversationActivity> {
        let mut state = self.lock()?;
        state.admission.record_measured_invocation_progress(
            invocation_ref,
            status,
            summary,
            published,
            progress,
        )?;
        latest_execution_activity(&state.admission, invocation_ref)
    }

    fn record_adapter_activity(
        &self,
        invocation_ref: &str,
        attempt_count: u64,
        event: AdapterActivityEvent,
    ) -> CwrResult<Option<ConversationActivity>> {
        let mut state = self.lock()?;
        if !state.admission.invocation_record(invocation_ref).is_some_and(|record|
            record.attempt_active && record.attempt_count == attempt_count) {
            return Ok(None);
        }
        match event {
            AdapterActivityEvent::ExecutionActivity { activity } => state.admission
                .append_execution_activity(invocation_ref,
                    ConversationActivityEvent::ExecutionActivity { activity }).map(Some),
            AdapterActivityEvent::ProviderThread {
                provider,
                thread_id,
            } => state
                .admission
                .append_execution_activity(
                    invocation_ref,
                    ConversationActivityEvent::ProviderThread {
                        provider,
                        thread_id,
                    },
                )
                .map(Some),
            AdapterActivityEvent::ProviderEvent {
                provider,
                method,
                data,
            } => state
                .admission
                .append_execution_activity(
                    invocation_ref,
                    ConversationActivityEvent::ProviderEvent {
                        provider,
                        method,
                        data,
                    },
                )
                .map(Some),
            AdapterActivityEvent::CompletedCommentary { commentary } => {
                match state.admission.record_commentary_progress(
                    invocation_ref,
                    attempt_count,
                    commentary,
                )? {
                    crate::v2026_09::conversation::RecordCommentaryProgressOutcome::Recorded(
                        activity,
                    ) => Ok(Some(activity)),
                    crate::v2026_09::conversation::RecordCommentaryProgressOutcome::Existing(_)
                    | crate::v2026_09::conversation::RecordCommentaryProgressOutcome::IgnoredStaleModelRun => Ok(None),
                }
            }
        }
    }
}
