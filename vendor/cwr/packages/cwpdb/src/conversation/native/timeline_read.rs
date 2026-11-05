use super::*;
use rusqlite::{params, OptionalExtension};
mod dependencies;
use dependencies::Dependencies;
impl NativeConversationStore {
    pub(super) fn timeline_page_facts(
        &mut self,
        request: &TimelineReadRequest,
        control: &ReadControl,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<TimelineReadFacts>> {
        self.check_scope(&request.scope)?;
        valid_ref(&request.conversation_ref)?;
        validate(request)?;
        control.check()?;
        let monitor = control.clone();
        let enabled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let monitoring = enabled.clone();
        self.connection
            .busy_timeout(std::time::Duration::from_millis(5))?;
        self.connection.progress_handler(
            if monitor.vm_steps.is_some() { 1 } else { 1000 },
            Some(move || {
                if let Some(counter) = &monitor.vm_steps {
                    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                monitoring.load(std::sync::atomic::Ordering::Acquire) && monitor.check().is_err()
            }),
        )?;
        let result = (|| loop {
            control.check()?;
            enabled.store(true, std::sync::atomic::Ordering::Release);
            let result: CwpdbResult<Option<TimelineReadFacts>> = (|| {
                let tx = self.connection.transaction()?;
                let selected = (|| {
                    authorizer.authorize(
                        &request.scope.context,
                        &request.conversation_ref,
                        false,
                    )?;
                    let Some(header) = resource::conversation_facts(
                        &tx,
                        &request.scope,
                        &request.conversation_ref,
                    )?
                    else {
                        return Ok(None);
                    };
                    if (!request.include_execution && header.parent_ref.is_some())
                        || request
                            .participant_actor_ref
                            .as_ref()
                            .is_some_and(|actor| !header.participant_actor_refs.contains(actor))
                    {
                        return Err(
                            "conversation is unavailable to the requesting participant".into()
                        );
                    }
                    let result = page(&tx, request, header, control)?;
                    authorizer.authorize(
                        &request.scope.context,
                        &request.conversation_ref,
                        false,
                    )?;
                    control.check()?;
                    Ok(Some(result))
                })();
                enabled.store(false, std::sync::atomic::Ordering::Release);
                match selected {
                    Ok(value) => {
                        tx.commit()?;
                        Ok(value)
                    }
                    Err(error) => {
                        tx.rollback()?;
                        Err(error)
                    }
                }
            })();
            if result.as_ref().err().and_then(|e|e.downcast_ref::<rusqlite::Error>()).is_some_and(|e| matches!(e,rusqlite::Error::SqliteFailure(code,_) if matches!(code.code,rusqlite::ErrorCode::DatabaseBusy|rusqlite::ErrorCode::DatabaseLocked))) { std::thread::sleep(std::time::Duration::from_millis(1));continue; }
            return result;
        })();
        self.connection.progress_handler(0, None::<fn() -> bool>)?;
        self.connection
            .busy_timeout(std::time::Duration::from_secs(5))?;
        control.check()?;
        result
    }
}
pub(super) fn validate(request: &TimelineReadRequest) -> CwpdbResult<()> {
    if request.first.is_none() && request.last.is_none() {
        return Err("conversation record page requires first or last".into());
    }
    if [request.first, request.last]
        .into_iter()
        .flatten()
        .any(|n| n > 500)
    {
        return Err("conversation record page size must not exceed 500".into());
    }
    if request
        .after
        .as_ref()
        .zip(request.before.as_ref())
        .is_some_and(|(a, b)| a.position >= b.position)
    {
        return Err("conversation record page boundaries do not define a forward range".into());
    }
    if request.max_bytes == 0 || request.max_bytes > MAX_WIRE_BYTES {
        return Err("invalid timeline response byte budget".into());
    }
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    if request.deadline_unix_ms <= now {
        return Err("conversation read operation exceeded its deadline".into());
    }
    Ok(())
}
pub(super) fn page(
    db: &Connection,
    request: &TimelineReadRequest,
    header: ResourceConversationFacts,
    control: &ReadControl,
) -> CwpdbResult<TimelineReadFacts> {
    let root = resource::one(db, &request.conversation_ref, "conversation_timeline_root")?;
    let revision = if let Some(root) = &root {
        pages::visibility::flat_revision(db, root)?
    } else {
        let props = read::get(db, &request.conversation_ref)?
            .ok_or("conversation disappeared")?
            .properties;
        props["timeline_revision"]
            .as_i64()
            .unwrap_or_else(|| props["commit_revision"].as_i64().unwrap_or(0))
            .to_string()
    };
    for cursor in [request.after.as_ref(), request.before.as_ref()]
        .into_iter()
        .flatten()
    {
        if (root.is_some() && cursor.revision.as_deref() != Some(revision.as_str()))
            || (root.is_none() && cursor.revision.as_deref().is_some_and(|r| r != revision))
        {
            return Err("conversation record cursor belongs to an earlier revision".into());
        }
    }
    let canonical = root.is_some();
    if let Some(root) = &root {
        pages::visibility::validate_walk(db, root)?;
    }
    let relation = if canonical {
        format!("{},flat(position,item_id) AS (SELECT ROW_NUMBER() OVER (ORDER BY path)-1,item_id FROM walk WHERE NOT EXISTS(SELECT 1 FROM edges content JOIN nodes body ON body.ref_id=content.target_id WHERE content.source_id=walk.item_id AND content.kind='conversation_timeline_message' AND json_type(body.props,'$.streaming')='object')), anchors(position,kind,reference,content_count) AS (SELECT flat.position,CASE content.kind WHEN 'conversation_timeline_message' THEN 'message' ELSE 'turn' END,reference.canonical_ref,(SELECT COUNT(*) FROM edges all_content WHERE all_content.source_id=flat.item_id AND all_content.kind IN ('conversation_timeline_message','conversation_timeline_invocation')) FROM flat LEFT JOIN edges content ON content.source_id=flat.item_id AND content.kind IN ('conversation_timeline_message','conversation_timeline_invocation') LEFT JOIN ref_bindings reference ON reference.ref_id=content.target_id)",pages::visibility::WALK)
    } else {
        "WITH anchors(position,kind,reference,content_count) AS (SELECT e.position,json_extract(e.props,'$.item_kind'),reference.canonical_ref,1 FROM edges e INDEXED BY edges_out JOIN ref_bindings reference ON reference.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='timeline_item' AND ?2 IS NOT NULL)".into()
    };
    let root_arg = root.as_deref().unwrap_or("");
    if canonical {
        let invalid: bool = db.query_row(
            &format!("{relation} SELECT EXISTS(SELECT 1 FROM anchors WHERE content_count!=1)"),
            params![request.conversation_ref, root_arg],
            |r| r.get(0),
        )?;
        if invalid {
            return Err("timeline item must point to exactly one content record".into());
        }
    }
    let total: i64 = db.query_row(
        &format!("{relation} SELECT COUNT(*) FROM anchors"),
        params![request.conversation_ref, root_arg],
        |row| row.get(0),
    )?;
    for cursor in [request.after.as_ref(), request.before.as_ref()]
        .into_iter()
        .flatten()
    {
        let exists: bool = db.query_row(
            &format!("{relation} SELECT EXISTS(SELECT 1 FROM anchors WHERE position=?3)"),
            params![request.conversation_ref, root_arg, cursor.position],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(
                "conversation record page boundary is not in the requested relation".into(),
            );
        }
    }
    let lower = request.after.as_ref().map_or(i64::MIN, |k| k.position);
    let upper = request.before.as_ref().map_or(i64::MAX, |k| k.position);
    let requested = request
        .first
        .unwrap_or_default()
        .max(request.last.unwrap_or_default());
    let direction = if request.first.is_some() {
        "ASC"
    } else {
        "DESC"
    };
    let sql=format!("{relation} SELECT position,kind,reference,content_count FROM anchors WHERE position>?3 AND position<?4 ORDER BY position {direction} LIMIT ?5");
    let mut statement = db.prepare(&sql)?;
    let mut anchors = statement
        .query_map(
            params![
                request.conversation_ref,
                root_arg,
                lower,
                upper,
                i64::try_from(requested + 1)?
            ],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )?
        .map(|row| {
            let (position, kind, reference, count) = row?;
            if count != 1 {
                return Err("timeline item must point to exactly one content record".into());
            }
            match kind.as_str() {
                "message" => Ok(TimelineAnchorFacts::Message {
                    position,
                    message_ref: reference,
                }),
                "turn" => Ok(TimelineAnchorFacts::Turn {
                    position,
                    invocation_ref: reference,
                }),
                _ => Err("timeline item has unsupported content".into()),
            }
        })
        .collect::<CwpdbResult<Vec<_>>>()?;
    let mut next = request.first.is_some_and(|n| anchors.len() > n);
    let mut previous = if let Some(first) = request.first {
        anchors.truncate(first);
        request.last.is_some_and(|n| anchors.len() > n)
    } else {
        let extra = request.last.is_some_and(|n| anchors.len() > n);
        anchors.truncate(request.last.unwrap_or_default());
        anchors.reverse();
        extra
    };
    if let Some(last) = request.last {
        if anchors.len() > last {
            anchors.drain(..anchors.len() - last);
            previous = true;
        }
    }
    if canonical {
        previous |= request.after.as_ref().is_some_and(|k| k.position > 0);
        next |= request
            .before
            .as_ref()
            .is_some_and(|k| k.position + 1 < total);
    } else {
        if let Some(after) = &request.after {
            previous |= db.query_row(
                &format!("{relation} SELECT EXISTS(SELECT 1 FROM anchors WHERE position<?3)"),
                params![request.conversation_ref, root_arg, after.position],
                |r| r.get::<_, bool>(0),
            )?;
        }
        if let Some(before) = &request.before {
            next |= db.query_row(
                &format!("{relation} SELECT EXISTS(SELECT 1 FROM anchors WHERE position>?3)"),
                params![request.conversation_ref, root_arg, before.position],
                |r| r.get::<_, bool>(0),
            )?;
        }
    }
    let (messages, activity) = Dependencies::new(
        db,
        &request.conversation_ref,
        root.as_deref(),
        control,
        request.max_bytes,
    )
    .read(&anchors)?;
    let facts = TimelineReadFacts {
        runtime_root_ref: request.scope.runtime_root_ref.clone(),
        conversation_ref: request.conversation_ref.clone(),
        commit_revision: runtime_metadata(db, &request.scope)?["commit_revision"]
            .as_i64()
            .ok_or("invalid commit revision")?
            .to_string(),
        conversation: header,
        revision,
        anchors,
        messages,
        activity,
        has_next_page: next,
        has_previous_page: previous,
        total_count: u64::try_from(total)?,
    };
    control.check()?;
    if serde_json::to_vec(&facts)?.len() > request.max_bytes {
        return Err("timeline page exceeds byte budget".into());
    }
    Ok(facts)
}

#[cfg(test)]
mod tests;
