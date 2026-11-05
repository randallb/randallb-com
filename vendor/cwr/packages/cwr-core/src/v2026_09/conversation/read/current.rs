//! Public protocol projection of finite facts from one cwpdb-owned snapshot.
use super::{invalid, ConversationReadOperation};
use crate::{v2026_09::conversation::backend::resource::project_resource_record, CwrResult};
mod types;
pub use types::*;

impl ConversationReadOperation<'_> {
    fn require_current_conversation(&self, conversation: &str) -> CwrResult<()> {
        self.ensure_active()?;
        if !self.finite_snapshot()?.conversation_exists(
            conversation,
            self.include_execution_conversations,
            None,
        )? {
            return invalid(format!("unknown conversation ref {conversation}"));
        }
        Ok(())
    }

    pub fn conversation_current(
        &self,
        conversation: &str,
    ) -> CwrResult<ConversationCurrentSnapshot> {
        self.require_current_conversation(conversation)?;
        let facts = self.finite_snapshot()?.current(conversation)?;
        let active_invocations = facts
            .into_iter()
            .map(|facts| {
                let reference = match &facts {
                    cwpdb_store::conversation::ResourceRecordFacts::Invocation {
                        record, ..
                    } => record["invocation_ref"]
                        .as_str()
                        .ok_or("invocation identity is missing")?
                        .to_owned(),
                    _ => return invalid("unexpected current resource kind"),
                };
                match project_resource_record(&reference, conversation, facts)? {
                    ConversationResource::Invocation { invocation } => Ok(*invocation),
                    _ => invalid("unexpected current resource projection"),
                }
            })
            .collect::<CwrResult<Vec<_>>>()?;
        self.ensure_active()?;
        Ok(ConversationCurrentSnapshot {
            position: self.snapshot_position(conversation)?,
            active_invocations,
        })
    }

    pub fn conversation_resource(
        &self,
        reference: &str,
    ) -> CwrResult<Option<ConversationResourceSnapshot>> {
        self.ensure_active()?;
        if !super::canonical_lookup_reference(reference) {
            return Ok(None);
        }
        let Some(facts) = self.finite_snapshot()?.resource(reference)? else {
            return Ok(None);
        };
        self.require_current_conversation(&facts.conversation_ref)?;
        let resource = project_resource_record(reference, &facts.conversation_ref, facts.resource)?;
        self.ensure_active()?;
        Ok(Some(ConversationResourceSnapshot {
            position: ConversationSnapshotPosition {
                runtime_root_ref: facts.runtime_root_ref,
                conversation_ref: facts.conversation_ref,
                commit_revision: facts.commit_revision,
            },
            resource,
        }))
    }

    pub(super) fn snapshot_position(
        &self,
        conversation: &str,
    ) -> CwrResult<ConversationSnapshotPosition> {
        let snapshot = self.finite_snapshot()?;
        Ok(ConversationSnapshotPosition {
            runtime_root_ref: snapshot.runtime_root_ref().into(),
            conversation_ref: conversation.into(),
            commit_revision: snapshot.commit_revision().into(),
        })
    }

    pub fn conversation_invocation(
        &self,
        conversation: &str,
        reference: &str,
    ) -> CwrResult<Option<ConversationInvocation>> {
        self.require_current_conversation(conversation)?;
        let Some(facts) = self
            .finite_snapshot()?
            .invocation(conversation, reference)?
        else {
            return Ok(None);
        };
        self.ensure_active()?;
        match project_resource_record(reference, conversation, facts)? {
            ConversationResource::Invocation { invocation } => Ok(Some(*invocation)),
            _ => invalid("unexpected invocation resource kind"),
        }
    }
}
