//! Inspector reads canonical tool operations and the invocation ledger.
use super::*;
use cwpdb_store::conversation::{
    InspectorCollection as StorageInspectorCollection, InspectorKey as StorageInspectorKey,
    InspectorReadRequest,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConversationInspectorCollection {
    Activities,
    Calls,
    Waits,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConversationInspectorKey {
    pub scope: String,
    pub position: i64,
    pub reference: String,
}
#[derive(Debug, Clone, Default)]
pub struct ConversationInspectorRequest {
    pub first: Option<usize>,
    pub last: Option<usize>,
    pub after: Option<ConversationInspectorKey>,
    pub before: Option<ConversationInspectorKey>,
}
#[derive(Debug, Clone)]
pub struct ConversationInspectorPage {
    pub entries: Vec<(ConversationInspectorKey, ConversationInspectorEntry)>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    /// Tool capture coverage is partial; the call/wait ledger is authoritative.
    pub complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationInspectorEntry {
    Activity {
        activity_ref: String,
        activity_kind: String,
        label: String,
        label_truncated: bool,
        state: String,
        active: bool,
        started_at: Option<u64>,
        duration_ms: Option<u64>,
        child_invocation_ref: Option<String>,
        details_available: bool,
    },
    Call {
        call_id: String,
        callee_invocation_ref: Option<String>,
    },
    Wait {
        call_id: String,
        awaited_invocation_ref: Option<String>,
        state: crate::v2026_09::invocation::InvocationWaitState,
    },
    File {
        path: String,
        path_truncated: bool,
        change_kind: String,
    },
}
#[derive(Debug, Clone)]
pub struct ConversationInspectorText {
    pub text: String,
    pub truncated: bool,
}
#[derive(Debug, Clone, Copy)]
pub enum ConversationInspectorTextField {
    Command,
    Output,
    Error,
    Diff,
}

impl ConversationInspectorRequest {
    fn validate(&self) -> CwrResult<usize> {
        let size = self.first.or(self.last).unwrap_or(50);
        if !(1..=100).contains(&size) || self.first.is_some() && self.last.is_some() {
            return invalid("Select first or last between 1 and 100");
        }
        if self
            .after
            .as_ref()
            .zip(self.before.as_ref())
            .is_some_and(|(a, b)| a >= b)
        {
            return invalid("Invalid inspector cursor range");
        }
        Ok(size)
    }
}
impl ConversationPublicationSnapshot {
    pub fn inspector_page(
        &self,
        invocation: &str,
        collection: ConversationInspectorCollection,
        request: &ConversationInspectorRequest,
    ) -> CwrResult<Option<ConversationInspectorPage>> {
        request.validate()?;
        self.read(|op| {
            let Some(root) = self.inspector_invocation_root(op, invocation)? else {
                return Ok(None);
            };
            if collection == ConversationInspectorCollection::Activities {
                let available = op
                    .finite_snapshot()?
                    .inspector_capture_available(invocation)?;
                if !available {
                    return Ok(None);
                }
            }
            let details = self.inspector_details_allowed(op, invocation)?;
            let query = match collection {
                ConversationInspectorCollection::Activities => {
                    StorageInspectorCollection::Activities
                }
                ConversationInspectorCollection::Calls => StorageInspectorCollection::Calls,
                ConversationInspectorCollection::Waits => StorageInspectorCollection::Waits,
            };
            let mut page =
                self.inspector_select(op, (invocation, &root), details, query, request, None)?;
            page.complete = collection != ConversationInspectorCollection::Activities;
            for (_, entry) in &mut page.entries {
                match entry {
                    ConversationInspectorEntry::Activity {
                        child_invocation_ref,
                        ..
                    } => self.hide_unavailable_invocation(op, child_invocation_ref)?,
                    ConversationInspectorEntry::Call {
                        callee_invocation_ref,
                        ..
                    } => self.hide_unavailable_invocation(op, callee_invocation_ref)?,
                    ConversationInspectorEntry::Wait {
                        awaited_invocation_ref,
                        ..
                    } => self.hide_unavailable_invocation(op, awaited_invocation_ref)?,
                    _ => {}
                }
            }
            Ok(Some(page))
        })
    }
    fn hide_unavailable_invocation(
        &self,
        op: &ConversationReadOperation<'_>,
        reference: &mut Option<String>,
    ) -> CwrResult<()> {
        if let Some(value) = reference.as_deref() {
            if self.inspector_invocation_root(op, value)?.is_none() {
                *reference = None;
            }
        }
        Ok(())
    }
    fn inspector_invocation_root(
        &self,
        op: &ConversationReadOperation<'_>,
        invocation: &str,
    ) -> CwrResult<Option<String>> {
        let Some(root) = op
            .finite_snapshot()?
            .inspector_invocation_root(invocation)?
        else {
            return Ok(None);
        };
        if !self.visible(op, &root)? {
            return Ok(None);
        }
        Ok(Some(root))
    }
    /// Transfer uses exactly the same visibility and payload policy as inspector reads.
    pub(in crate::v2026_09::conversation) fn can_export_inspector_details(
        &self,
        invocation: &str,
    ) -> CwrResult<bool> {
        self.read(|op| {
            if self.inspector_invocation_root(op, invocation)?.is_none() {
                return Ok(false);
            }
            self.inspector_details_allowed(op, invocation)
        })
    }

    // Participants can see basic work. Payloads are restricted to the direct
    // requester/target or requester of a retained ancestor; parentage grants no mutation authority.
    fn inspector_details_allowed(
        &self,
        op: &ConversationReadOperation<'_>,
        invocation: &str,
    ) -> CwrResult<bool> {
        op.finite_snapshot()?
            .inspector_details_allowed(invocation, &self.actor)
    }

    fn inspector_select(
        &self,
        op: &ConversationReadOperation<'_>,
        owner: (&str, &str),
        details: bool,
        collection: StorageInspectorCollection,
        request: &ConversationInspectorRequest,
        activity: Option<&str>,
    ) -> CwrResult<ConversationInspectorPage> {
        let (invocation, root) = owner;
        let size = request.validate()?;
        let key = |key: &ConversationInspectorKey| StorageInspectorKey {
            scope: key.scope.clone(),
            position: key.position,
            reference: key.reference.clone(),
        };
        let backwards = request.last.is_some();
        let facts = op
            .finite_snapshot()?
            .inspector_page(&InspectorReadRequest {
                invocation_ref: invocation.into(),
                conversation_ref: root.into(),
                actor_ref: self.actor.clone(),
                collection,
                activity_ref: activity.map(str::to_owned),
                details,
                after: request.after.as_ref().map(key),
                before: request.before.as_ref().map(key),
                size,
                backwards,
                remaining_bytes: (512usize * 1024).saturating_sub(self.inspector_bytes.get()),
            })?;
        let mut entries = Vec::new();
        for fact in facts {
            op.ensure_active()?;
            self.reserve_inspector_bytes(fact.serialized_bytes)?;
            entries.push((
                ConversationInspectorKey {
                    scope: fact.key.scope,
                    position: fact.key.position,
                    reference: fact.key.reference,
                },
                serde_json::from_value(fact.payload)?,
            ));
        }
        let more = entries.len() > size;
        entries.truncate(size);
        if backwards {
            entries.reverse();
        }
        Ok(ConversationInspectorPage {
            entries,
            has_next_page: !backwards && more,
            has_previous_page: backwards && more,
            complete: false,
        })
    }
    fn reserve_inspector_bytes(&self, bytes: usize) -> CwrResult<()> {
        let total = self.inspector_bytes.get().saturating_add(bytes);
        if total > 512 * 1024 {
            return invalid("Inspector selection exceeds 512 KiB response budget");
        }
        self.inspector_bytes.set(total);
        Ok(())
    }
}

mod details;
