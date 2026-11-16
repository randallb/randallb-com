//! Finite retained history reads, pinned across one protocol operation.
use super::*;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
pub(crate) struct CanonicalHistoryReader {
    backend: Arc<Mutex<ConversationBackend>>,
    reference: String,
    participant: Option<String>,
    include_execution: bool,
    control: ReadControl,
    revision: RefCell<Option<String>>,
}
impl ConversationAdmissionStore {
    pub(crate) fn canonical_history_reader(
        &self,
        reference: &str,
        participant: Option<String>,
        include_execution: bool,
        control: ReadControl,
    ) -> CanonicalHistoryReader {
        CanonicalHistoryReader {
            backend: self.backend.clone(),
            reference: reference.into(),
            participant,
            include_execution,
            control,
            revision: RefCell::new(None),
        }
    }
}
impl CanonicalHistoryReader {
    pub(crate) fn ensure_active(&self) -> CwrResult<()> {
        self.control.check()
    }
    pub(crate) fn facts(&self, selection: HistorySelection) -> CwrResult<HistoryReadFacts> {
        self.control.check()?;
        let mut backend = loop {
            self.control.check()?;
            match self.backend.try_lock() {
                Ok(b) => break b,
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                Err(_) => return invalid("conversation backend lock poisoned"),
            }
        };
        let scope = backend.scope.clone();
        let remaining = self
            .control
            .deadline
            .saturating_duration_since(Instant::now());
        let deadline = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .checked_add(remaining)
            .ok_or("read deadline overflow")?;
        let request = HistoryReadRequest {
            scope: scope.clone(),
            conversation_ref: self.reference.clone(),
            selection,
            expected_commit_revision: self.revision.borrow().clone(),
            participant_actor_ref: self.participant.clone(),
            include_execution: self.include_execution,
            deadline_unix_ms: i64::try_from(deadline.as_millis())?,
            max_bytes: MAX_WIRE_BYTES,
        };
        let ConversationBackend {
            storage,
            authorizer,
            ..
        } = &mut *backend;
        let facts = storage
            .history_read(&request, &self.control, authorizer.as_ref())?
            .ok_or_else(|| invalid_error(format!("unknown conversation ref {}", self.reference)))?;
        if facts.runtime_root_ref != scope.runtime_root_ref
            || facts.conversation_ref != self.reference
            || facts.commit_revision.parse::<i64>().is_err()
            || request
                .expected_commit_revision
                .as_ref()
                .is_some_and(|r| r != &facts.commit_revision)
        {
            return invalid("history snapshot identity mismatch");
        }
        if (!self.include_execution && facts.conversation.parent_ref.is_some())
            || self
                .participant
                .as_ref()
                .is_some_and(|a| !facts.conversation.participant_actor_refs.contains(a))
        {
            return invalid("conversation is unavailable to the requesting participant");
        }
        *self.revision.borrow_mut() = Some(facts.commit_revision.clone());
        self.control.check()?;
        Ok(facts)
    }
}
