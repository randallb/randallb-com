//! Finite metadata facts for a participant-authorized publication selection.
use super::*;
use rusqlite::{params, OptionalExtension};

impl NativeReadSnapshot {
    pub fn participant_visible(&self, conversation: &str, actor: &str) -> CwpdbResult<bool> {
        self.control.check()?;
        valid_ref(conversation)?;
        valid_ref(actor)?;
        let result = self.connection.query_row("SELECT EXISTS(SELECT 1 FROM conversations c JOIN ref_bindings r ON r.canonical_ref=c.conversation_ref JOIN edges p ON p.source_id=r.ref_id AND p.kind='participant' JOIN ref_bindings a ON a.ref_id=p.target_id WHERE c.conversation_ref=?1 AND c.parent_conversation_ref IS NULL AND a.canonical_ref=?2)", params![conversation, actor], |r| r.get(0))?;
        self.control.check()?;
        Ok(result)
    }

    pub fn root_conversation_presentation(
        &self,
        conversation: &str,
    ) -> CwpdbResult<Option<(Option<String>, bool)>> {
        self.control.check()?;
        valid_ref(conversation)?;
        Ok(self.connection.query_row("SELECT title,archived FROM conversations WHERE conversation_ref=?1 AND parent_conversation_ref IS NULL", [conversation], |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
    }

    pub fn fork_source(&self, conversation: &str) -> CwpdbResult<Option<(String, String)>> {
        self.control.check()?;
        valid_ref(conversation)?;
        Ok(self.connection.query_row("SELECT json_extract(CAST(payload_json AS TEXT),'$.event.source_conversation_ref'), json_extract(CAST(payload_json AS TEXT),'$.event.source_message_ref') FROM activities WHERE conversation_ref=?1 AND activity_kind='conversation_forked_from' ORDER BY position DESC LIMIT 1", [conversation], |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
    }

    pub fn owner_context_ref(&self, conversation: &str) -> CwpdbResult<Option<String>> {
        self.control.check()?;
        valid_ref(conversation)?;
        Ok(self.connection.query_row(
            "SELECT owner_context_ref FROM conversations WHERE conversation_ref=?1",
            [conversation],
            |r| r.get(0),
        )?)
    }

    pub fn timeline_item_conversation(&self, item: &str) -> CwpdbResult<Option<String>> {
        self.control.check()?;
        valid_ref(item)?;
        Ok(self.connection.query_row("WITH RECURSIVE owners(ref_id) AS (SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1 UNION SELECT e.source_id FROM edges e JOIN owners o ON e.target_id=o.ref_id WHERE e.kind IN ('conversation_timeline_item','conversation_timeline_child','conversation_timeline_root')) SELECT r.canonical_ref FROM owners o JOIN ref_bindings r ON r.ref_id=o.ref_id JOIN conversations c ON c.conversation_ref=r.canonical_ref WHERE c.parent_conversation_ref IS NULL LIMIT 1", [item], |r| r.get(0)).optional()?)
    }

    pub fn actor_identity(&self, actor: &str) -> CwpdbResult<Option<Value>> {
        self.control.check()?;
        valid_ref(actor)?;
        let result: Option<String> = self.connection.query_row("SELECT json_extract(props,'$.record') FROM nodes WHERE kind='local_actor_identity' AND json_extract(props,'$.record.actor_ref')=?1 LIMIT 1", [actor], |r| r.get(0)).optional()?;
        result.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
    }

    pub fn actor_candidate(&self, actor: &str) -> CwpdbResult<bool> {
        self.control.check()?;
        valid_ref(actor)?;
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM ref_bindings WHERE canonical_ref=?1) AND NOT EXISTS(SELECT 1 FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE r.canonical_ref=?1 AND n.kind NOT IN ('actor','coworker'))", [actor], |r| r.get(0))?)
    }

    pub fn actor_conversations(&self, actor: &str) -> CwpdbResult<Vec<String>> {
        self.control.check()?;
        valid_ref(actor)?;
        self.references("SELECT c.conversation_ref FROM conversations c JOIN edges p ON p.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=c.conversation_ref) AND p.kind='participant' WHERE p.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND c.parent_conversation_ref IS NULL LIMIT 50001", actor)
    }

    pub fn actor_resources(&self, actor: &str) -> CwpdbResult<Vec<String>> {
        self.control.check()?;
        valid_ref(actor)?;
        self.references("SELECT r.canonical_ref FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE (n.kind='message' AND (json_extract(n.props,'$.record.author_actor_ref')=?1 OR json_extract(n.props,'$.streaming.message.author_actor_ref')=?1 OR EXISTS(SELECT 1 FROM json_each(json_extract(n.props,'$.record.addressed_actor_refs')) a WHERE a.value=?1))) OR (n.kind='invocation' AND (json_extract(n.props,'$.record.requester_actor_ref')=?1 OR json_extract(n.props,'$.record.target_actor_ref')=?1)) ORDER BY r.canonical_ref LIMIT 50001", actor)
    }

    pub fn artifact_exists(&self, reference: &str) -> CwpdbResult<bool> {
        self.control.check()?;
        valid_ref(reference)?;
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM nodes WHERE kind='visualization_artifact' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1))", [reference], |r| r.get(0))?)
    }

    pub fn artifact_messages(&self, reference: &str) -> CwpdbResult<Vec<String>> {
        self.control.check()?;
        valid_ref(reference)?;
        self.references("SELECT DISTINCT m.message_ref FROM messages m,json_each(json_extract(CAST(m.payload_json AS TEXT),'$.visualizations')) v WHERE json_extract(v.value,'$.artifact_ref')=?1 ORDER BY m.message_ref LIMIT 50001", reference)
    }

    /// The caller first establishes a visible associated message in this view.
    pub fn artifact_html(&self, reference: &str) -> CwpdbResult<Option<String>> {
        self.control.check()?;
        valid_ref(reference)?;
        Ok(self.connection.query_row("SELECT json_extract(props,'$.record.html') FROM nodes WHERE kind='visualization_artifact' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", [reference], |r| r.get(0)).optional()?)
    }

    pub fn invocation_reply_ref(&self, reference: &str) -> CwpdbResult<Option<String>> {
        self.control.check()?;
        valid_ref(reference)?;
        Ok(self.connection.query_row("SELECT COALESCE(json_extract(n.props,'$.record.reply_message_ref'), (SELECT json_extract(a.props,'$.record.reply_message_ref') FROM edges e JOIN nodes a ON a.ref_id=e.source_id WHERE e.target_id=n.ref_id AND e.kind='field:invocation_ref' AND a.kind='admission' LIMIT 1)) FROM nodes n WHERE n.ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND n.kind='invocation'", [reference], |r| r.get(0)).optional()?.flatten())
    }

    pub fn invocation_fork_destination(
        &self,
        reference: &str,
        intrinsic_actor: &str,
    ) -> CwpdbResult<Option<String>> {
        self.control.check()?;
        valid_ref(reference)?;
        Ok(self.connection.query_row("SELECT json_extract(props,'$.record.responses[#-1].result.conversation_ref') FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='invocation' AND json_extract(props,'$.record.action_name')='fork--conversation' AND json_extract(props,'$.record.target_actor_ref')=?2 AND json_extract(props,'$.record.state')='completed' AND json_extract(props,'$.record.responses[#-1].result.kind')='conversation_forked'", params![reference, intrinsic_actor], |r| r.get(0)).optional()?.flatten())
    }

    pub fn invocation_stop_facts(
        &self,
        reference: &str,
        intrinsic_actor: &str,
    ) -> CwpdbResult<Option<InvocationStopFacts>> {
        self.control.check()?;
        valid_ref(reference)?;
        Ok(self.connection.query_row("SELECT json_extract(props,'$.record.input.invocation_ref'), COALESCE(json_extract(props,'$.record.responses[#-1].result.kind'),json_extract(props,'$.record.responses[#-1].error.details.kind')), COALESCE(json_extract(props,'$.record.responses[#-1].result.invocation_ref'),json_extract(props,'$.record.responses[#-1].error.details.invocation_ref')), json_extract(props,'$.record.state') FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='invocation' AND json_extract(props,'$.record.action_name')='stop--invocation' AND json_extract(props,'$.record.target_actor_ref')=?2", params![reference, intrinsic_actor], |r| Ok(InvocationStopFacts { input: r.get(0)?, result: r.get(1)?, resolved: r.get(2)?, state: r.get(3)? })).optional()?)
    }

    fn references(&self, sql: &str, reference: &str) -> CwpdbResult<Vec<String>> {
        let result = self
            .connection
            .prepare(sql)?
            .query_map([reference], |r| r.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        if result.len() > 50000 || serde_json::to_vec(&result)?.len() > MAX_WIRE_BYTES {
            return Err("publication relationship work limit exceeded".into());
        }
        self.control.check()?;
        Ok(result)
    }
}

impl NativeReadSnapshot {
    pub fn invocation_visible(&self, root: &str, invocation: &str) -> CwpdbResult<bool> {
        self.control.check()?;
        valid_ref(root)?;
        valid_ref(invocation)?;
        let result = resource::visible(
            &self.connection,
            Some(root),
            invocation,
            "conversation_timeline_invocation",
        )?;
        self.control.check()?;
        Ok(result)
    }
}
