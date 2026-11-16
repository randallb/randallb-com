//! Retained legacy layout selection without expanding work dependencies.
use super::*;
use rusqlite::params;
impl NativeReadSnapshot {
    fn legacy_context(
        &self,
        reference: &str,
        include_execution: bool,
    ) -> CwpdbResult<pages::PageContext> {
        self.control.check()?;
        valid_ref(reference)?;
        let mut context = pages::context(&self.connection, &self.scope, reference)?
            .ok_or("unknown conversation")?;
        if !include_execution && context.conversation.parent_ref.is_some() {
            return Err("unknown conversation".into());
        }
        if context.root.is_some() {
            return Err(
                "conversation has a canonical timeline; select its retained layout again".into(),
            );
        }
        let node = read::get(&self.connection, reference)?.ok_or("conversation disappeared")?;
        if let Some(revision) = node.properties["timeline_revision"].as_i64() {
            context.revision = revision.to_string();
        }
        Ok(context)
    }
    fn legacy_key(&self, reference: &str, revision: &str, key: &PositionCursor) -> CwpdbResult<()> {
        if key.revision.as_deref() != Some(revision) {
            return Err("conversation record cursor belongs to an earlier revision".into());
        }
        let exists:bool=self.connection.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='timeline_item' AND position=?2)",params![reference,key.position],|row|row.get(0))?;
        if !exists {
            return Err(
                "conversation record page boundary is not in the requested relation".into(),
            );
        }
        Ok(())
    }
    pub fn legacy_timeline_page(
        &self,
        reference: &str,
        window: &PositionWindow,
        include_execution: bool,
    ) -> CwpdbResult<TimelineReadFacts> {
        let context = self.legacy_context(reference, include_execution)?;
        let (count, reverse) = match (window.first, window.last) {
            (Some(n), None) if n > 0 && n <= 500 => (n, false),
            (None, Some(n)) if n > 0 && n <= 500 => (n, true),
            _ => {
                return Err(
                    "legacy timeline page requires exactly one positive first or last".into(),
                )
            }
        };
        for key in [window.after.as_ref(), window.before.as_ref()]
            .into_iter()
            .flatten()
        {
            self.legacy_key(reference, &context.revision, key)?;
        }
        let lower = window.after.as_ref().map(|k| k.position);
        let upper = window.before.as_ref().map(|k| k.position);
        if lower.zip(upper).is_some_and(|(l, u)| l >= u) {
            return Err("conversation record page boundaries do not define a forward range".into());
        }
        let direction = if reverse { "DESC" } else { "ASC" };
        let mut statement=self.connection.prepare(&format!("SELECT e.position,json_extract(e.props,'$.item_kind'),r.canonical_ref FROM edges e JOIN ref_bindings r ON r.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='timeline_item' AND e.position>?2 AND e.position<?3 ORDER BY e.position {direction} LIMIT ?4"))?;
        let rows = statement
            .query_map(
                params![
                    reference,
                    lower.unwrap_or(i64::MIN),
                    upper.unwrap_or(i64::MAX),
                    i64::try_from(count + 1)?
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let overflow = rows.len() > count;
        let mut rows = rows.into_iter().take(count).collect::<Vec<_>>();
        if reverse {
            rows.reverse();
        }
        let first = rows.first().map(|r| r.0).or(upper);
        let last = rows.last().map(|r| r.0).or(lower);
        let exists = |low, high| {
            self.connection.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='timeline_item' AND position>?2 AND position<?3)",params![reference,low,high],|row|row.get::<_,bool>(0))
        };
        let previous = (rows.is_empty() && lower.is_some())
            || (reverse && overflow)
            || first
                .map(|p| exists(i64::MIN, p))
                .transpose()?
                .unwrap_or(false);
        let next = (rows.is_empty() && upper.is_some())
            || (!reverse && overflow)
            || last
                .map(|p| exists(p, i64::MAX))
                .transpose()?
                .unwrap_or(false);
        let anchors = rows
            .into_iter()
            .map(|(position, kind, reference)| match kind.as_str() {
                "message" => Ok(TimelineAnchorFacts::Message {
                    position,
                    message_ref: reference,
                }),
                "turn" => Ok(TimelineAnchorFacts::Turn {
                    position,
                    invocation_ref: reference,
                }),
                _ => Err("timeline item has unsupported content".into()),
            })
            .collect::<CwpdbResult<Vec<_>>>()?;
        let facts = TimelineReadFacts {
            runtime_root_ref: self.scope.runtime_root_ref.clone(),
            conversation_ref: reference.into(),
            commit_revision: self.commit_revision.clone(),
            conversation: context.conversation,
            revision: context.revision,
            anchors,
            messages: Vec::new(),
            activity: Vec::new(),
            has_next_page: next,
            has_previous_page: previous,
            total_count: 0,
        };
        self.control.check()?;
        if serde_json::to_vec(&facts)?.len() > MAX_WIRE_BYTES {
            return Err("legacy timeline page exceeds byte budget".into());
        }
        Ok(facts)
    }
    pub fn legacy_timeline_message(
        &self,
        reference: &str,
        key: &PositionCursor,
        message: &str,
        include_execution: bool,
    ) -> CwpdbResult<Option<Value>> {
        let context = self.legacy_context(reference, include_execution)?;
        self.legacy_key(reference, &context.revision, key)?;
        valid_ref(message)?;
        let exists:bool=self.connection.query_row("SELECT EXISTS(SELECT 1 FROM edges i JOIN edges m ON m.source_id=i.source_id AND m.kind='timeline_message' AND m.target_id=i.target_id WHERE i.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND i.kind='timeline_item' AND i.position=?2 AND json_extract(i.props,'$.item_kind')='message' AND i.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3))",params![reference,key.position,message],|row|row.get(0))?;
        let record = if exists {
            resource::message(&self.connection, reference, message)?
        } else {
            None
        };
        self.control.check()?;
        if serde_json::to_vec(&record)?.len() > MAX_WIRE_BYTES {
            return Err("legacy message exceeds byte budget".into());
        }
        Ok(record)
    }
}
