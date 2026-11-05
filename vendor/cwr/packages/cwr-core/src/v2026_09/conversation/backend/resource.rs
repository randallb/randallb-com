//! Public resource projection from one finite canonical backend snapshot.
use super::*;
use crate::adapter::ExecutionPhase;
use crate::v2026_09::invocation::ActionInvocationState;

fn execution_projection(
    facts: ResourceExecutionFacts,
    record: &Value,
) -> CwrResult<ConversationExecution> {
    let state: ActionInvocationState = serde_json::from_value(record["state"].clone())?;
    let default = match state {
        ActionInvocationState::AwaitingPerson | ActionInvocationState::Waiting => {
            ExecutionPhase::Waiting
        }
        ActionInvocationState::Admitted => ExecutionPhase::Queued,
        _ => ExecutionPhase::Working,
    };
    let phase = if state == ActionInvocationState::Running && facts.attempt_active {
        facts
            .latest_phase_event
            .map(|event| -> CwrResult<ExecutionPhase> {
                if event["state"] == "completed" {
                    Ok(ExecutionPhase::Working)
                } else {
                    Ok(serde_json::from_value(event["phase"].clone())?)
                }
            })
            .transpose()?
            .unwrap_or(default)
    } else {
        default
    };
    let started = facts.started_at_unix_ms.or_else(|| {
        (facts.generation == 1)
            .then(|| record["started_at_unix_ms"].as_u64())
            .flatten()
    });
    Ok(ConversationExecution {
        generation: facts.generation,
        attempt_active: facts.attempt_active,
        phase,
        started_at_unix_ms: started,
        latest_activity_at_unix_ms: facts
            .latest_activity_at_unix_ms
            .into_iter()
            .chain(started)
            .max(),
        progress: facts.unpublished_progress,
        live_output: None,
    })
}
pub(in crate::v2026_09::conversation) fn project_resource_record(
    reference: &str,
    conversation_ref: &str,
    facts: ResourceRecordFacts,
) -> CwrResult<ConversationResource> {
    Ok(match facts {
        ResourceRecordFacts::Message {
            record,
            streaming,
            occurred_at_unix_ms,
        } => {
            let message: ConversationMessage = serde_json::from_value(record)?;
            if message.message_ref != reference {
                return invalid("resource message identity mismatch");
            }
            ConversationResource::Message {
                message: Box::new(message),
                streaming,
                occurred_at_unix_ms,
            }
        }
        ResourceRecordFacts::Invocation {
            mut record,
            child_invocation_refs,
            wait_records,
            latest_reply,
            execution,
        } => {
            if record["invocation_ref"] != reference
                || record["conversation_ref"] != conversation_ref
            {
                return invalid("resource invocation identity mismatch");
            }
            let current_execution = execution
                .map(|facts| execution_projection(facts, &record))
                .transpose()?;
            let object = record
                .as_object_mut()
                .ok_or("invalid resource invocation facts")?;
            object.insert(
                "child_invocation_refs".into(),
                serde_json::to_value(child_invocation_refs)?,
            );
            object.insert("waits".into(), serde_json::to_value(wait_records)?);
            object.insert("latest_reply".into(), latest_reply.unwrap_or(Value::Null));
            object.insert(
                "current_execution".into(),
                serde_json::to_value(current_execution)?,
            );
            let invocation: ConversationInvocation = serde_json::from_value(record)?;
            ConversationResource::Invocation {
                invocation: Box::new(invocation),
            }
        }
    })
}

#[cfg(test)]
impl ConversationAdmissionStore {
    pub(crate) fn canonical_conversation_resource(
        &self,
        reference: &str,
    ) -> CwrResult<Option<ConversationResourceSnapshot>> {
        let mut reader = self.read_store()?;
        reader
            .begin_operation(std::time::Duration::from_secs(2))?
            .conversation_resource(reference)
    }
}
