//! Named retained-history operations on this selection's existing WAL snapshot.
use super::*;
impl NativeReadSnapshot {
    pub fn history_read(
        &self,
        reference: &str,
        selection: HistorySelection,
        participant: Option<String>,
        include_execution: bool,
    ) -> CwpdbResult<Option<HistoryReadFacts>> {
        self.control.check()?;
        valid_ref(reference)?;
        let remaining = self
            .control
            .deadline
            .saturating_duration_since(std::time::Instant::now());
        let deadline = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .checked_add(remaining)
            .ok_or("read deadline overflow")?;
        let request = HistoryReadRequest {
            scope: self.scope.clone(),
            conversation_ref: reference.into(),
            selection,
            expected_commit_revision: Some(self.commit_revision.clone()),
            participant_actor_ref: participant,
            include_execution,
            deadline_unix_ms: i64::try_from(deadline.as_millis())?,
            max_bytes: MAX_WIRE_BYTES,
        };
        super::super::history_read::validate(&request)?;
        let Some(header) = resource::conversation_facts(&self.connection, &self.scope, reference)?
        else {
            return Ok(None);
        };
        if (!include_execution && header.parent_ref.is_some())
            || request
                .participant_actor_ref
                .as_ref()
                .is_some_and(|a| !header.participant_actor_refs.contains(a))
        {
            return Err("conversation is unavailable to the requesting participant".into());
        }
        let facts =
            super::super::history_read::page(&self.connection, &request, header, &self.control)?;
        self.control.check()?;
        Ok(Some(facts))
    }
}
