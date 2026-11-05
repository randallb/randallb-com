//! CWR decides whether finite retained facts require the eager recovery pass.
use super::*;
use crate::v2026_09::invocation::{ActionInvocationState, InvocationOrigin, InvocationWaitState};

impl ConversationBackend {
    pub(super) fn needs_history_recovery(
        &mut self,
        state: &ConversationRuntimeState,
    ) -> CwrResult<bool> {
        if !state.streaming_runs.is_empty()
            || state.invocation_ledger.invocations().any(|record| {
                matches!(
                    record.state,
                    ActionInvocationState::Waiting | ActionInvocationState::Stopping
                )
            })
            || state.invocation_ledger.waits().iter().any(|wait| {
                matches!(
                    wait.state,
                    InvocationWaitState::Pending
                        | InvocationWaitState::Ready
                        | InvocationWaitState::Resuming
                )
            })
        {
            return Ok(true);
        }
        let mut facts = BTreeMap::new();
        for conversation in state.conversations.iter() {
            let reference = &conversation.conversation_ref;
            let status = self
                .storage
                .history_status(&self.scope, reference, self.authorizer.as_ref())?
                .ok_or("retained conversation history status is unavailable")?;
            if status.revision != self.revision {
                return invalid("conversation revision_conflict during recovery inspection");
            }
            if (conversation.parent_conversation_ref.is_none() && !status.has_timeline_root)
                || status.has_streaming
                || status.has_legacy_context
            {
                return Ok(true);
            }
            facts.insert(reference.as_str(), status);
        }
        for admission in &state.admissions {
            let Some(reference) = admission.invocation_ref.as_deref() else {
                continue;
            };
            let Some(record) = state.invocation_ledger.invocation(reference) else {
                return Ok(true);
            };
            if !matches!(record.origin, InvocationOrigin::Ingress { .. }) {
                return Ok(true);
            }
            if !record.state.is_terminal() {
                continue;
            }
            let response = record
                .terminal_response()
                .ok_or("terminal invocation has no response")?;
            if admission.state != terminal::admission_state(response)? {
                return Ok(true);
            }
            let Some(execution) = admission.execution_conversation_ref.as_deref() else {
                return Ok(true);
            };
            if !facts
                .get(execution)
                .is_some_and(|facts| facts.terminal_activity)
            {
                return Ok(true);
            }
            let expects_reply = response.status.is_completed()
                || record.action_name != RESPOND_TO_MESSAGE_ACTION_NAME;
            let Some(reply) = admission.reply_message_ref.as_deref() else {
                if expects_reply {
                    return Ok(true);
                }
                continue;
            };
            if !self.storage.contains_member(
                &self.scope,
                &admission.conversation_ref,
                MembershipKind::Message,
                reply,
                self.authorizer.as_ref(),
            )? {
                return Ok(true);
            }
        }
        // Membership probes are exact reads. Fence the final one against the
        // bootstrap token so a concurrent commit cannot create a mixed view.
        self.storage.read(
            &ReadRequest {
                scope: self.scope.clone(),
                selection: Selection::Records {
                    kind: RecordKind::Runtime,
                },
                continuation: None,
                page_size: 1,
                max_bytes: MAX_WIRE_BYTES,
                expected_revision: Some(self.revision.clone()),
            },
            self.authorizer.as_ref(),
        )?;
        Ok(false)
    }
}
