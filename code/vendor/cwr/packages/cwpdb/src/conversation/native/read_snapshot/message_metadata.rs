//! Exact retained associations used for public message presentation.
use super::*;
use rusqlite::{params, OptionalExtension};

impl NativeReadSnapshot {
    pub fn streaming_message_metadata(
        &self,
        message: &str,
    ) -> CwpdbResult<(Option<String>, String)> {
        self.control.check()?;
        valid_ref(message)?;
        Ok(self.connection.query_row("SELECT json_extract(props,'$.streaming.phase'), json_extract(props,'$.streaming.invocation_ref') FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", [message], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }

    pub fn message_action_invocation(
        &self,
        conversation: &str,
        message: &str,
    ) -> CwpdbResult<Option<String>> {
        self.control.check()?;
        valid_ref(conversation)?;
        valid_ref(message)?;
        let mut statement = self.connection.prepare("SELECT r.canonical_ref FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE n.kind='invocation' AND json_extract(n.props,'$.record.conversation_ref')=?1 AND (json_extract(n.props,'$.record.reply_message_ref')=?2 OR json_extract(n.props,'$.record.source_message_ref')=?2 OR EXISTS(SELECT 1 FROM edges link JOIN nodes admission ON admission.ref_id=link.source_id WHERE link.target_id=n.ref_id AND link.kind='field:invocation_ref' AND admission.kind='admission' AND json_extract(admission.props,'$.record.reply_message_ref')=?2)) LIMIT 2")?;
        let refs = statement
            .query_map(params![conversation, message], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        self.control.check()?;
        Ok(match refs.as_slice() {
            [reference] => Some(reference.clone()),
            _ => None,
        })
    }

    pub fn progress_activity(
        &self,
        invocation: &str,
        source_activity: &str,
    ) -> CwpdbResult<Option<Value>> {
        self.control.check()?;
        valid_ref(invocation)?;
        valid_ref(source_activity)?;
        let bytes: Option<Vec<u8>> = self.connection.query_row("SELECT a.payload_json FROM activities a JOIN execution_conversation_refs x ON x.conversation_ref=a.conversation_ref WHERE a.activity_ref=?1 AND x.invocation_ref=?2", params![source_activity, invocation], |r| r.get(0)).optional()?;
        bytes
            .map(|bytes| Ok(serde_json::from_slice(&bytes)?))
            .transpose()
    }

    pub fn progress_in_latest_attempt(
        &self,
        invocation: &str,
        sequence: Option<u64>,
    ) -> CwpdbResult<bool> {
        self.control.check()?;
        valid_ref(invocation)?;
        let Some(sequence) = sequence else {
            return Ok(false);
        };
        let sequence = i64::try_from(sequence)?;
        let (generation, boundary): (i64, Option<i64>) = self.connection.query_row("SELECT COALESCE(json_extract(props,'$.record.attempt_count'),0), json_extract(props,'$.record.attempt_started_after_activity_sequence') FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", [invocation], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(boundary.map_or(generation == 1, |boundary| sequence > boundary))
    }

    pub fn latest_work_activity(
        &self,
        invocation: &str,
        activity: &str,
    ) -> CwpdbResult<Option<Value>> {
        self.control.check()?;
        valid_ref(invocation)?;
        let value: Option<String> = self.connection.query_row("SELECT json_extract(CAST(a.payload_json AS TEXT),'$.event.activity') FROM activities a JOIN execution_conversation_refs x ON x.conversation_ref=a.conversation_ref WHERE x.invocation_ref=?1 AND a.activity_kind='work_update' AND json_extract(CAST(a.payload_json AS TEXT),'$.event.activity.activity_ref')=?2 ORDER BY a.position DESC LIMIT 1", params![invocation, activity], |r| r.get(0)).optional()?;
        value
            .map(|value| Ok(serde_json::from_str(&value)?))
            .transpose()
    }
}
