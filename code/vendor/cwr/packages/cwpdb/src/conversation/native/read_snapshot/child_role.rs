//! Fixed predicates for commentary and final replies in one frozen selection.
use super::*;
use rusqlite::{params, OptionalExtension};
impl NativeReadSnapshot {
    pub fn invocation_target_source(
        &self,
        invocation: &str,
    ) -> CwpdbResult<Option<(String, Option<String>)>> {
        self.control.check()?;
        valid_ref(invocation)?;
        Ok(self.connection.query_row("SELECT json_extract(n.props,'$.record.target_actor_ref'), json_extract(n.props,'$.record.source_message_ref') FROM nodes n WHERE n.kind='invocation' AND n.ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", [invocation], |row| Ok((row.get(0)?, row.get(1)?))).optional()?)
    }
    pub fn message_is_commentary(&self, message: &str, invocation: &str) -> CwpdbResult<bool> {
        self.control.check()?;
        valid_ref(message)?;
        valid_ref(invocation)?;
        let value: Option<i64> = self.connection.query_row("SELECT (json_extract(n.props,'$.streaming.phase')='commentary' AND json_extract(n.props,'$.streaming.invocation_ref')=?2) OR json_extract(n.props,'$.record.progress.invocation_ref')=?2 FROM nodes n WHERE n.kind='message' AND n.ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", params![message, invocation], |row| row.get(0)).optional()?.flatten();
        Ok(value.unwrap_or_default() != 0)
    }
    pub fn message_is_final_reply(
        &self,
        message: &str,
        target: &str,
        source: Option<&str>,
    ) -> CwpdbResult<bool> {
        self.control.check()?;
        valid_ref(message)?;
        valid_ref(target)?;
        if let Some(source) = source {
            valid_ref(source)?;
        }
        let value: Option<i64> = self.connection.query_row("SELECT CASE WHEN json_type(n.props,'$.streaming')='object' THEN json_extract(n.props,'$.streaming.phase')<>'commentary' AND json_extract(n.props,'$.streaming.message.author_actor_ref')=?2 AND (?3 IS NULL OR json_extract(n.props,'$.streaming.message.reply_to_message_ref')=?3) ELSE json_type(n.props,'$.record.progress') IS NULL AND COALESCE(json_extract(n.props,'$.record.action.kind'),'')<>'call' AND json_extract(n.props,'$.record.author_actor_ref')=?2 AND (?3 IS NULL OR json_extract(n.props,'$.record.reply_to_message_ref')=?3) END FROM nodes n WHERE n.kind='message' AND n.ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", params![message, target, source], |row| row.get(0)).optional()?.flatten();
        Ok(value.unwrap_or_default() != 0)
    }
}
