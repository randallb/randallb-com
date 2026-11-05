impl ConversationAdmissionStore {
    pub(crate) fn admit_provider_question(&mut self, graph: &ResolvedCwrGraph, ingress: &ConversationIngress, binding: &str, metadata: Value) -> CwrResult<ConversationAdmissionOutcome> {
        let mut prepared = self.prepare(graph, ingress, Some(binding))?;
        if let ConversationPreparationOutcome::New(turn) = &mut prepared {
            let invocation = turn.invocation.as_mut().ok_or("person response actions are unavailable")?;
            invocation.input["provider_question"] = metadata;
            invocation.output_schema["properties"]["answers_response"] = json!({"type":"object"});
            if let Some(execution) = &mut turn.execution_conversation {
                for activity in &mut execution.activity {
                    if let ConversationActivityEvent::TurnStarted { invocation: snapshot, .. } = &mut activity.event {
                        *snapshot = Some(context::invocation_snapshot(invocation)?);
                    }
                }
            }
        }
        self.commit(prepared)
    }

    /// A provider question owns a live RPC. Never offer it after process recovery.
    pub(crate) fn clear_provider_questions(&mut self, all: bool) -> CwrResult<()> {
        let stale = self.invocation_records().filter(|r| !r.state.is_terminal())
            .filter_map(|r| r.input.pointer("/provider_question/caller").and_then(Value::as_str).map(|owner| (r, owner)))
            .filter(|(_, owner)| all || self.invocation_record(owner).is_none_or(|r| r.state.is_terminal()))
            .map(|(r, _)| r.invocation_ref.clone()).collect::<Vec<_>>();
        for reference in stale {
            self.publish_root_stop(&reference, crate::v2026_09::invocation::InvocationStopRequest::new(current_unix_ms()), "Question is no longer waiting for an answer.".into())?;
        }
        Ok(())
    }
}
