//! Typed control metadata reads the existing action invocation, never raw payloads.
use super::*;
use crate::v2026_09::conversation::ConversationMessage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationStopDisposition {
    Pending,
    Accepted,
    TooLate,
    Rejected,
}

#[derive(Debug, Clone)]
pub struct InvocationStopControl {
    pub target: Option<Box<super::super::ConversationInvocation>>,
    pub disposition: InvocationStopDisposition,
}

impl ConversationPublicationSnapshot {
    /// Read the published reply through the authorized resource view.
    pub fn invocation_result_message(
        &self,
        reference: &str,
    ) -> CwrResult<Option<Box<ConversationMessage>>> {
        self.read(|op| {
            if self.resource_in(op, reference)?.is_none() {
                return Ok(None);
            }
            let reply = op.finite_snapshot()?.invocation_reply_ref(reference)?;
            let Some(reply) = reply else {
                return Ok(None);
            };
            Ok(match self.resource_in(op, &reply)?.map(|s| s.resource) {
                Some(ConversationResource::Message {
                    message,
                    streaming: false,
                    ..
                }) => Some(message),
                _ => None,
            })
        })
    }

    pub fn invocation_fork_destination(&self, reference: &str) -> CwrResult<Option<String>> {
        self.read(|op| {
            if self.resource_in(op, reference)?.is_none() {
                return Ok(None);
            }
            let destination = op.finite_snapshot()?.invocation_fork_destination(
                reference,
                crate::action_contract::INTRINSIC_CWR_ACTOR_REF,
            )?;
            match destination {
                Some(destination) if self.visible(op, &destination)? => Ok(Some(destination)),
                _ => Ok(None),
            }
        })
    }

    pub fn invocation_stop_control(
        &self,
        reference: &str,
    ) -> CwrResult<Option<InvocationStopControl>> {
        self.read(|op| {
            if !matches!(
                self.resource_in(op, reference)?.map(|value| value.resource),
                Some(ConversationResource::Invocation { .. })
            ) {
                return Ok(None);
            }
            // Select only the known stop action's routing and typed response fields.
            let row = op.finite_snapshot()?.invocation_stop_facts(
                reference,
                crate::action_contract::INTRINSIC_CWR_ACTOR_REF,
            )?;
            let Some(cwpdb_store::conversation::InvocationStopFacts {
                input,
                result,
                resolved,
                state,
            }) = row
            else {
                return Ok(None);
            };
            let disposition = match result.as_deref() {
                Some("invocation_stopped") => InvocationStopDisposition::Accepted,
                Some("invocation_stop_too_late") => InvocationStopDisposition::TooLate,
                Some("invocation_stop_failed") => InvocationStopDisposition::Rejected,
                _ if matches!(state.as_str(), "failed" | "blocked" | "stopped") => {
                    InvocationStopDisposition::Rejected
                }
                _ => InvocationStopDisposition::Pending,
            };
            let target = match resolved.or(input) {
                Some(target) => match self.resource_in(op, &target)?.map(|snapshot| *snapshot) {
                    Some(ConversationResourceSnapshot {
                        resource: ConversationResource::Invocation { invocation },
                        ..
                    }) => Some(invocation),
                    _ => None,
                },
                None => None,
            };
            Ok(Some(InvocationStopControl {
                target,
                disposition,
            }))
        })
    }
}
