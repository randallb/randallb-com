//! Tool inspector queries are selected by finite enums in the storage owner.
use super::*;
use rusqlite::{params, OptionalExtension};
include!("inspector/queries.rs");
const FILE_ENTRIES: &str = "SELECT e.position AS position,CAST(json_extract(owner.props,'$.record.sequence') AS TEXT)||':'||e.position AS reference,
 json_object('kind','file','path',substr(json_extract(n.props,'$.record.path'),1,4096),
 'path_truncated',json(CASE WHEN length(json_extract(n.props,'$.record.path'))>4096 THEN 'true' ELSE 'false' END),
 'change_kind',substr(json_extract(n.props,'$.record.kind'),1,256)) AS payload
 FROM edges e JOIN nodes n ON n.ref_id=e.target_id AND n.kind='tool_file' JOIN nodes owner ON owner.ref_id=e.source_id
 WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?9)
 AND e.kind='tool_file' AND ?1 IS NOT NULL AND ?2 IS NOT NULL AND ?3=1";
fn entries(collection: InspectorCollection) -> &'static str {
    match collection {
        InspectorCollection::Activities => ACTIVITY_ENTRIES,
        InspectorCollection::Calls => CALL_ENTRIES,
        InspectorCollection::Waits => WAIT_ENTRIES,
        InspectorCollection::Files => FILE_ENTRIES,
    }
}
impl NativeReadSnapshot {
    pub fn inspector_capture_available(&self, invocation: &str) -> CwpdbResult<bool> {
        self.control.check()?;
        valid_ref(invocation)?;
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes n ON n.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='tool_attempt' AND n.kind='tool_attempt' AND json_extract(n.props,'$.record.capture_version')=1)", [invocation], |r| r.get(0))?)
    }
    pub fn inspector_details_allowed(&self, invocation: &str, actor: &str) -> CwpdbResult<bool> {
        self.control.check()?;
        valid_ref(invocation)?;
        valid_ref(actor)?;
        Ok(self.connection.query_row("WITH RECURSIVE lineage(ref) AS (SELECT ?1 UNION SELECT json_extract(n.props,'$.record.parent_invocation_ref') FROM lineage l JOIN ref_bindings r ON r.canonical_ref=l.ref JOIN nodes n ON n.ref_id=r.ref_id AND n.kind='invocation' WHERE json_extract(n.props,'$.record.parent_invocation_ref') IS NOT NULL) SELECT EXISTS(SELECT 1 FROM lineage l JOIN ref_bindings r ON r.canonical_ref=l.ref JOIN nodes n ON n.ref_id=r.ref_id AND n.kind='invocation' WHERE json_extract(n.props,'$.record.requester_actor_ref')=?2 OR (l.ref=?1 AND json_extract(n.props,'$.record.target_actor_ref')=?2))", params![invocation, actor], |r| r.get(0))?)
    }
    pub fn inspector_invocation_root(&self, invocation: &str) -> CwpdbResult<Option<String>> {
        self.control.check()?;
        let Some(resource) = self.resource(invocation)? else {
            return Ok(None);
        };
        Ok(
            matches!(resource.resource, ResourceRecordFacts::Invocation { .. })
                .then_some(resource.conversation_ref),
        )
    }
    pub fn inspector_scope(
        &self,
        actor: &str,
        invocation: &str,
        collection: InspectorCollection,
        activity: Option<&str>,
    ) -> String {
        crate::derived_ref(
            "inspector_scope",
            &[
                actor,
                invocation,
                activity.unwrap_or(""),
                entries(collection),
            ],
        )
    }
    fn inspector_owner(
        &self,
        invocation: &str,
        conversation: &str,
        actor: &str,
        details: bool,
    ) -> CwpdbResult<()> {
        if self.inspector_invocation_root(invocation)?.as_deref() != Some(conversation)
            || !self.participant_visible(conversation, actor)?
        {
            return Err("inspector invocation is unavailable".into());
        }
        if details && !self.inspector_details_allowed(invocation, actor)? {
            return Err("inspector details are unavailable".into());
        }
        Ok(())
    }
    pub fn inspector_page(
        &self,
        request: &InspectorReadRequest,
    ) -> CwpdbResult<Vec<InspectorEntryFacts>> {
        self.control.check()?;
        if !(1..=100).contains(&request.size) || request.remaining_bytes > 512 * 1024 {
            return Err("invalid inspector page budget".into());
        }
        self.inspector_owner(
            &request.invocation_ref,
            &request.conversation_ref,
            &request.actor_ref,
            request.details,
        )?;
        if request.collection == InspectorCollection::Files
            && (!request.details || request.activity_ref.is_none())
        {
            return Err("file inspector requires activity details".into());
        }
        let scope = self.inspector_scope(
            &request.actor_ref,
            &request.invocation_ref,
            request.collection,
            request.activity_ref.as_deref(),
        );
        let entries = format!(
            "SELECT * FROM ({}) WHERE (?9 IS NULL OR 1=1)",
            entries(request.collection)
        );
        for key in [&request.after, &request.before].into_iter().flatten() {
            if key.scope != scope {
                return Err("Invalid inspector cursor scope".into());
            }
            let exists: bool = self.connection.query_row(&format!("WITH entries AS ({entries}) SELECT EXISTS(SELECT 1 FROM entries WHERE position=?4 AND reference=?5)"), params![request.invocation_ref, request.conversation_ref, request.details, key.position, key.reference, None::<i64>, None::<String>, None::<i64>, request.activity_ref], |r| r.get(0))?;
            if !exists {
                return Err("Inspector cursor entry unavailable; request a fresh page".into());
            }
        }
        let order = if request.backwards { "DESC" } else { "ASC" };
        let sql = format!("WITH entries AS ({entries}) SELECT position,reference,length(CAST(payload AS BLOB))+length(CAST(reference AS BLOB))+64,payload FROM entries WHERE (?4 IS NULL OR (position,reference)>(?4,?5)) AND (?6 IS NULL OR (position,reference)<(?6,?7)) ORDER BY position {order},reference {order} LIMIT ?8");
        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query(params![
            request.invocation_ref,
            request.conversation_ref,
            request.details,
            request.after.as_ref().map(|k| k.position),
            request.after.as_ref().map(|k| &k.reference),
            request.before.as_ref().map(|k| k.position),
            request.before.as_ref().map(|k| &k.reference),
            i64::try_from(request.size + 1)?,
            request.activity_ref
        ])?;
        let mut result = Vec::new();
        let mut total = 0usize;
        while let Some(row) = rows.next()? {
            self.control.check()?;
            let bytes = usize::try_from(row.get::<_, i64>(2)?)?;
            total = total.saturating_add(bytes);
            if total > request.remaining_bytes {
                return Err("Inspector selection exceeds 512 KiB response budget".into());
            }
            let payload: String = row.get(3)?;
            result.push(InspectorEntryFacts {
                key: InspectorKey {
                    scope: scope.clone(),
                    position: row.get(0)?,
                    reference: row.get(1)?,
                },
                payload: serde_json::from_str(&payload)?,
                serialized_bytes: bytes,
            });
        }
        Ok(result)
    }
    pub fn inspector_activity_sequence(
        &self,
        invocation: &str,
        root: &str,
        activity: &str,
    ) -> CwpdbResult<Option<i64>> {
        self.control.check()?;
        valid_ref(invocation)?;
        valid_ref(root)?;
        valid_ref(activity)?;
        Ok(self.connection.query_row(&format!("WITH records AS ({ACTIVITY_RECORDS}) SELECT json_extract(props,'$.record.sequence') FROM records WHERE activity_ref=?4 AND ?3=1 ORDER BY sequence DESC LIMIT 1"), params![invocation, root, true, activity], |r| r.get::<_, Option<i64>>(0)).optional()?.flatten())
    }
    pub fn inspector_exit_code(
        &self,
        invocation: &str,
        root: &str,
        actor: &str,
        activity: &str,
    ) -> CwpdbResult<Option<i64>> {
        self.inspector_owner(invocation, root, actor, true)?;
        Ok(self.connection.query_row(&format!("WITH records AS ({ACTIVITY_RECORDS}) SELECT json_extract(props,'$.record.activity.exit_code') FROM records WHERE activity_ref=?4 AND ?3=1 ORDER BY sequence DESC LIMIT 1"), params![invocation, root, true, activity], |r| r.get::<_, Option<i64>>(0)).optional()?.flatten())
    }
    pub fn inspector_text(
        &self,
        request: &InspectorTextRequest,
    ) -> CwpdbResult<Option<InspectorTextFacts>> {
        self.inspector_owner(
            &request.invocation_ref,
            &request.conversation_ref,
            &request.actor_ref,
            true,
        )?;
        let value: Option<(Option<Vec<u8>>, Option<i64>)> = if matches!(
            request.field,
            InspectorTextField::Diff
        ) {
            let key = request
                .file
                .as_ref()
                .ok_or("File identity required for diff")?;
            if key.position < 0 {
                return Err("Invalid file position".into());
            }
            let Some(sequence) = self.inspector_activity_sequence(
                &request.invocation_ref,
                &request.conversation_ref,
                &request.activity_ref,
            )?
            else {
                return Ok(None);
            };
            if key.reference != format!("{sequence}:{}", key.position) {
                return Err("File entry changed; request a fresh page".into());
            }
            self.connection.query_row("SELECT substr(CAST(json_extract(n.props,'$.record.diff') AS BLOB),1,?3),length(CAST(json_extract(n.props,'$.record.diff') AS BLOB)) FROM edges e JOIN nodes n ON n.ref_id=e.target_id AND n.kind='tool_file' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='tool_file' AND e.position=?2", params![request.activity_ref, key.position, 16384], |r| Ok((r.get(0)?, r.get(1)?))).optional()?
        } else {
            let field = match request.field {
                InspectorTextField::Command => "command",
                InspectorTextField::Output => "output",
                InspectorTextField::Error => "error",
                InspectorTextField::Diff => unreachable!(),
            };
            let path = format!("$.record.activity.{field}");
            self.connection.query_row(&format!("WITH records AS ({ACTIVITY_RECORDS}) SELECT substr(CAST(json_extract(props,'{path}') AS BLOB),1,?5),length(CAST(json_extract(props,'{path}') AS BLOB)) FROM records WHERE activity_ref=?4 AND ?3=1 ORDER BY sequence DESC LIMIT 1"), params![request.invocation_ref, request.conversation_ref, true, request.activity_ref, 16384], |r| Ok((r.get(0)?, r.get(1)?))).optional()?
        };
        self.control.check()?;
        Ok(match value {
            Some((Some(bytes), Some(full_bytes))) => Some(InspectorTextFacts { bytes, full_bytes }),
            _ => None,
        })
    }
}
