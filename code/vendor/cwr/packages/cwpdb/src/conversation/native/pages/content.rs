use super::*;
#[derive(Clone)]
pub(in crate::conversation::native) struct Key {
    pub(in crate::conversation::native) position: i64,
    pub(in crate::conversation::native) reference: String,
}
pub(in crate::conversation::native) fn validate(request: &ContentPageRequest) -> CwpdbResult<()> {
    if request.first.is_none() && request.last.is_none() {
        return Err("conversation record page requires first or last".into());
    }
    if [request.first, request.last]
        .into_iter()
        .flatten()
        .any(|count| count > 500)
    {
        return Err("conversation record page size must not exceed500".into());
    }
    if request
        .after
        .as_ref()
        .zip(request.before.as_ref())
        .is_some_and(|(after, before)| after.position >= before.position)
    {
        return Err("conversation record page boundaries do not define a forward range".into());
    }
    Ok(())
}
fn exists(
    db: &Connection,
    relation: &str,
    request: &ContentPageRequest,
    root: &str,
    lower: i64,
    upper: i64,
) -> CwpdbResult<bool> {
    let sql = format!(
        "{relation} SELECT EXISTS(SELECT 1 FROM messages WHERE position>?3 AND position<?4)"
    );
    Ok(db.query_row(
        &sql,
        params![request.conversation_ref, root, lower, upper],
        |row| row.get(0),
    )?)
}
pub(in crate::conversation::native) fn window(
    db: &Connection,
    relation: &str,
    request: &ContentPageRequest,
    root: &str,
) -> CwpdbResult<(Vec<Key>, bool, bool, u64)> {
    for cursor in [request.after.as_ref(), request.before.as_ref()]
        .into_iter()
        .flatten()
    {
        let sql = format!("{relation} SELECT EXISTS(SELECT 1 FROM messages WHERE position=?3)");
        let valid: bool = db.query_row(
            &sql,
            params![request.conversation_ref, root, cursor.position],
            |row| row.get(0),
        )?;
        if !valid {
            return Err(
                "conversation record page boundary is not in the requested relation".into(),
            );
        }
    }
    let total: i64 = db.query_row(
        &format!("{relation} SELECT COUNT(*) FROM messages"),
        params![request.conversation_ref, root],
        |row| row.get(0),
    )?;
    let requested = request.first.map_or_else(
        || request.last.unwrap_or_default(),
        |first| first.max(request.last.unwrap_or_default()),
    );
    let direction = if request.first.is_some() {
        "ASC"
    } else {
        "DESC"
    };
    let lower = request
        .after
        .as_ref()
        .map_or(i64::MIN, |cursor| cursor.position);
    let upper = request
        .before
        .as_ref()
        .map_or(i64::MAX, |cursor| cursor.position);
    let sql=format!("{relation} SELECT position,message_ref FROM messages WHERE position>?3 AND position<?4 ORDER BY position {direction} LIMIT ?5");
    let mut statement = db.prepare(&sql)?;
    let mut keys = statement
        .query_map(
            params![
                request.conversation_ref,
                root,
                lower,
                upper,
                i64::try_from(requested + 1)?
            ],
            |row| {
                Ok(Key {
                    position: row.get(0)?,
                    reference: row.get(1)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let next = request.first.is_some_and(|first| keys.len() > first);
    let mut previous = if let Some(first) = request.first {
        keys.truncate(first);
        request.last.is_some_and(|last| keys.len() > last)
    } else {
        let previous = request.last.is_some_and(|last| keys.len() > last);
        keys.truncate(request.last.unwrap_or_default());
        keys.reverse();
        previous
    };
    if let Some(last) = request.last {
        if keys.len() > last {
            keys.drain(..keys.len() - last);
            previous = true;
        }
    }
    let previous = if request.last.is_some() {
        previous
    } else if let Some(first) = keys.first() {
        exists(db, relation, request, root, lower, first.position)?
    } else {
        false
    };
    let next = if request.first.is_some() {
        next
    } else if let Some(last) = keys.last() {
        exists(db, relation, request, root, last.position, upper)?
    } else {
        false
    };
    Ok((keys, next, previous, u64::try_from(total)?))
}
pub(super) fn page(
    db: &Connection,
    request: &ContentPageRequest,
    context: PageContext,
) -> CwpdbResult<ContentPageFacts> {
    let canonical = context.root.is_some();
    if canonical {
        for cursor in [request.after.as_ref(), request.before.as_ref()]
            .into_iter()
            .flatten()
        {
            if cursor.revision.as_deref() != Some(context.revision.as_str()) {
                return Err("conversation record cursor belongs to an earlier revision".into());
            }
        }
        visibility::validate_walk(db, context.root.as_deref().unwrap())?;
    }
    let relation = visibility::message_relation(canonical);
    let root = context.root.as_deref().unwrap_or("");
    let (keys, has_next_page, has_previous_page, total_count) =
        window(db, &relation, request, root)?;
    let mut facts = ContentPageFacts {
        runtime_root_ref: request.scope.runtime_root_ref.clone(),
        conversation_ref: request.conversation_ref.clone(),
        commit_revision: context.commit_revision,
        conversation: context.conversation,
        canonical,
        revision: context.revision,
        items: Vec::new(),
        has_next_page,
        has_previous_page,
        total_count,
    };
    let mut bytes = serde_json::to_vec(&facts)?.len();
    if bytes > request.max_bytes {
        return Err("conversation content page exceeds byte budget".into());
    }
    for key in keys {
        let record = resource::message(db, &request.conversation_ref, &key.reference)?
            .ok_or("selected content message disappeared from snapshot")?;
        if record["message_ref"] != key.reference {
            return Err("selected content message identity mismatch".into());
        }
        let occurred:Option<i64>=db.query_row("SELECT json_extract(node.props,'$.record.occurred_at_unix_ms') FROM edges lookup INDEXED BY edges_in JOIN nodes node ON node.ref_id=lookup.source_id AND node.kind='activity' JOIN edges membership ON membership.target_id=node.ref_id AND membership.kind='activity' WHERE lookup.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND lookup.kind='field:event.message_ref' AND membership.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND json_extract(node.props,'$.record.event.kind')='message_recorded' ORDER BY membership.position LIMIT 1",params![key.reference,request.conversation_ref],|row|row.get(0)).optional()?.flatten();
        let item = ContentItemFacts {
            position: key.position,
            record,
            occurred_at_unix_ms: occurred,
        };
        bytes = bytes
            .checked_add(serde_json::to_vec(&item)?.len() + usize::from(!facts.items.is_empty()))
            .ok_or("content byte budget overflow")?;
        if bytes > request.max_bytes {
            return Err("conversation content page exceeds byte budget".into());
        }
        facts.items.push(item);
    }
    if serde_json::to_vec(&facts)?.len() > request.max_bytes {
        return Err("conversation content page exceeds byte budget".into());
    }
    Ok(facts)
}
