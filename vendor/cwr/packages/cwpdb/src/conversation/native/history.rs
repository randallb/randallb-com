use super::*;
use rusqlite::{params, OptionalExtension};
impl NativeConversationStore {
    pub(super) fn history_status_facts(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<HistoryStatus>> {
        self.check_scope(scope)?;
        valid_ref(reference)?;
        let tx = self.connection.transaction()?;
        authorizer.authorize(&scope.context, reference, false)?;
        let conversation:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE r.canonical_ref=?1 AND n.kind='conversation')",[reference],|row|row.get(0))?;
        if !conversation {
            return Ok(None);
        }
        let schema:Option<String>=tx.query_row("SELECT s.canonical_ref FROM nodes n LEFT JOIN ref_bindings s ON s.ref_id=n.schema_id WHERE n.ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[reference],|row|row.get(0))?;
        if schema.as_deref() != Some(crate::schema_ref("conversation").as_str()) {
            return Err("unsupported conversation schema".into());
        }
        let invalid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM edges e INDEXED BY edges_out LEFT JOIN ref_bindings s ON s.ref_id=e.schema_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind IN ('message','activity','conversation_timeline_root') AND (s.canonical_ref IS NULL OR s.canonical_ref!=?2 OR e.position<0))",params![reference,crate::EDGE_SCHEMA_REF],|row|row.get(0))?;
        if invalid {
            return Err("invalid retained CWR history relationship schema or position".into());
        }
        let has_timeline_root:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='conversation_timeline_root')",[reference],|row|row.get(0))?;
        let has_streaming:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM edges e INDEXED BY edges_out JOIN nodes n ON n.ref_id=e.target_id AND n.kind='message' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='message' AND json_type(n.props,'$.streaming')='object')",[reference],|row|row.get(0))?;
        let has_legacy_context:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM edges e INDEXED BY edges_out JOIN nodes n ON n.ref_id=e.target_id AND n.kind='activity' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='activity' AND (json_type(n.props,'$.record.event.invocation.resumed_host_call') IS NOT NULL OR EXISTS(SELECT 1 FROM json_tree(n.props,'$.record.event.invocation.thread') nested WHERE nested.key='messages')))",[reference],|row|row.get(0))?;
        let terminal_activity:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM edges e INDEXED BY edges_out JOIN nodes n ON n.ref_id=e.target_id AND n.kind='activity' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='activity' AND json_extract(n.props,'$.record.event.kind') IN ('turn_completed','turn_failed','turn_stopped'))",[reference],|row|row.get(0))?;
        let revision = current_revision(&tx, scope)?.0;
        authorizer.authorize(&scope.context, reference, false)?;
        tx.commit()?;
        Ok(Some(HistoryStatus {
            has_timeline_root,
            has_streaming,
            has_legacy_context,
            terminal_activity,
            revision,
        }))
    }
    pub(super) fn contains_member_facts(
        &mut self,
        scope: &Scope,
        source: &str,
        kind: MembershipKind,
        target: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<bool> {
        self.check_scope(scope)?;
        valid_ref(source)?;
        valid_ref(target)?;
        let tx = self.connection.transaction()?;
        authorizer.authorize(&scope.context, source, false)?;
        authorizer.authorize(&scope.context, target, false)?;
        let edge:Option<(Option<String>,i64)>=tx.query_row("SELECT schema.canonical_ref,e.position FROM edges e LEFT JOIN ref_bindings schema ON schema.ref_id=e.schema_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind=?2 AND e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3)",params![source,kind.as_str(),target],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        let mut present = false;
        if let Some((schema, position)) = edge {
            if schema.as_deref() != Some(crate::EDGE_SCHEMA_REF) || position < 0 {
                return Err("invalid retained CWR membership schema or position".into());
            }
            present = match kind {
                MembershipKind::Message | MembershipKind::Activity => {
                    let expected = if kind == MembershipKind::Message {
                        "message"
                    } else {
                        "activity"
                    };
                    tx.query_row("SELECT EXISTS(SELECT 1 FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind=?2)",params![target,expected],|row|row.get(0))?
                }
                _ => true,
            };
        }
        authorizer.authorize(&scope.context, source, false)?;
        authorizer.authorize(&scope.context, target, false)?;
        tx.commit()?;
        Ok(present)
    }
}
