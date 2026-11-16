//! Indexed finite operations on this selection's existing snapshot.
use super::*;
use rusqlite::OptionalExtension;
impl NativeReadSnapshot {
    pub fn content_message(
        &self,
        reference: &str,
        message: &str,
        include_execution: bool,
    ) -> CwpdbResult<Option<ContentItemFacts>> {
        let Some(mut facts) = self.history_read(
            reference,
            HistorySelection::ArchiveMessage {
                message_ref: message.into(),
            },
            None,
            include_execution,
        )?
        else {
            return Ok(None);
        };
        let item = facts.items.pop().ok_or("content point is missing")?;
        let record = item
            .record
            .ok_or("retained content record exceeds wire budget")?;
        let occurred=self.connection.query_row("SELECT json_extract(n.props,'$.record.occurred_at_unix_ms') FROM edges link INDEXED BY edges_in CROSS JOIN nodes n ON n.ref_id=link.source_id AND n.kind='activity' CROSS JOIN edges membership INDEXED BY edges_in ON membership.target_id=n.ref_id AND membership.kind='activity' WHERE link.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND link.kind='field:event.message_ref' AND membership.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND json_extract(n.props,'$.record.event.kind')='message_recorded' AND json_extract(n.props,'$.record.event.message_ref')=?1 ORDER BY membership.position LIMIT 1",rusqlite::params![message,reference],|r|r.get(0)).optional()?.flatten();
        self.control.check()?;
        Ok(Some(ContentItemFacts {
            position: item.position,
            record,
            occurred_at_unix_ms: occurred,
        }))
    }

    pub fn content_positions(
        &self,
        reference: &str,
        window: &PositionWindow,
        include_execution: bool,
    ) -> CwpdbResult<Option<HistoryReadFacts>> {
        self.history_read(
            reference,
            HistorySelection::ContentPositions {
                after: window.after.clone(),
                before: window.before.clone(),
                first: window.first,
                last: window.last,
            },
            None,
            include_execution,
        )
    }
    pub fn completion_page(
        &self,
        reference: &str,
        window: &CompletionWindow,
        include_execution: bool,
    ) -> CwpdbResult<Option<CompletionPageFacts>> {
        self.control.check()?;
        valid_ref(reference)?;
        if window.after < 0
            || window.through.is_some_and(|upper| upper < window.after)
            || !(1..=200).contains(&window.first)
        {
            return Err("invalid completion history window".into());
        }
        let Some(context) = pages::context(&self.connection, &self.scope, reference)? else {
            return Ok(None);
        };
        if !include_execution && context.conversation.parent_ref.is_some() {
            return Ok(None);
        }
        let request = CompletionPageRequest {
            scope: self.scope.clone(),
            conversation_ref: reference.into(),
            after_sequence: window.after,
            through_sequence: window.through,
            cursor_revision: window.revision.clone(),
            first: window.first,
            max_bytes: MAX_WIRE_BYTES,
        };
        let result = pages::completions::page(&self.connection, &request, context)?;
        self.control.check()?;
        Ok(Some(result))
    }
    pub fn timeline_page(
        &self,
        reference: &str,
        window: &PositionWindow,
        include_execution: bool,
    ) -> CwpdbResult<Option<TimelineReadFacts>> {
        self.control.check()?;
        valid_ref(reference)?;
        let Some(header) = resource::conversation_facts(&self.connection, &self.scope, reference)?
        else {
            return Ok(None);
        };
        if !include_execution && header.parent_ref.is_some() {
            return Ok(None);
        }
        let remaining = self
            .control
            .deadline
            .saturating_duration_since(std::time::Instant::now());
        let deadline = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .checked_add(remaining)
            .ok_or("read deadline overflow")?;
        let request = TimelineReadRequest {
            scope: self.scope.clone(),
            conversation_ref: reference.into(),
            after: window.after.clone(),
            before: window.before.clone(),
            first: window.first,
            last: window.last,
            participant_actor_ref: None,
            include_execution,
            deadline_unix_ms: i64::try_from(deadline.as_millis())?,
            max_bytes: MAX_WIRE_BYTES,
        };
        super::super::timeline_read::validate(&request)?;
        let result =
            super::super::timeline_read::page(&self.connection, &request, header, &self.control)?;
        self.control.check()?;
        Ok(Some(result))
    }
}
