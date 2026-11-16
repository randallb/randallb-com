use super::*;
fn visibility_filter(canonical: bool) -> &'static str {
    if canonical {
        "AND completion.target_id IN (WITH RECURSIVE connections(id) AS (SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2 UNION SELECT child.target_id FROM connections parent JOIN edges item ON item.source_id=parent.id AND item.kind='conversation_timeline_item' JOIN edges child ON child.source_id=item.target_id AND child.kind='conversation_timeline_child' LIMIT 50001) SELECT content.target_id FROM connections JOIN edges item ON item.source_id=connections.id AND item.kind='conversation_timeline_item' JOIN edges content ON content.source_id=item.target_id AND content.kind='conversation_timeline_invocation')"
    } else {
        "AND ?2 IS NOT NULL"
    }
}
pub(in crate::conversation::native) fn page(
    db: &Connection,
    request: &CompletionPageRequest,
    context: PageContext,
) -> CwpdbResult<CompletionPageFacts> {
    let canonical = context.root.is_some();
    if canonical
        && request.after_sequence > 0
        && request.cursor_revision.as_deref() != Some(context.revision.as_str())
    {
        return Err("conversation cursor belongs to an earlier canonical timeline revision".into());
    }
    let filter = visibility_filter(canonical);
    let root = context.root.as_deref().unwrap_or("");
    let upper=match request.through_sequence {Some(upper)=>upper,None=>db.query_row(&format!("SELECT COALESCE(MAX(completion.position),0) FROM edges completion INDEXED BY edges_out WHERE completion.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND completion.kind='completed_invocation' {filter}"),params![request.conversation_ref,root],|row|row.get(0))?};
    let sql=format!("SELECT reference.canonical_ref,completion.position FROM edges completion INDEXED BY edges_out JOIN ref_bindings reference ON reference.ref_id=completion.target_id WHERE completion.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND completion.kind='completed_invocation' {filter} AND completion.position>?3 AND completion.position<=?4 ORDER BY completion.position LIMIT ?5");
    let mut statement = db.prepare(&sql)?;
    let mut items = statement
        .query_map(
            params![
                request.conversation_ref,
                root,
                request.after_sequence,
                upper,
                i64::try_from(request.first + 1)?
            ],
            |row| {
                Ok(SummaryCompletion {
                    invocation_ref: row.get(0)?,
                    sequence: row.get(1)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut next = items.len() > request.first;
    items.truncate(request.first);
    while serde_json::to_vec(&items)?.len() > 256 * 1024 {
        items.pop();
        next = true;
    }
    if next && items.is_empty() {
        return Err("completion reference exceeds response byte limit".into());
    }
    let next_after = items
        .last()
        .map_or(request.after_sequence, |item| item.sequence);
    let facts = CompletionPageFacts {
        runtime_root_ref: request.scope.runtime_root_ref.clone(),
        conversation_ref: request.conversation_ref.clone(),
        commit_revision: context.commit_revision,
        conversation: context.conversation,
        canonical,
        revision: context.revision,
        items,
        through_sequence: upper,
        next_after,
        has_next_page: next,
    };
    if serde_json::to_vec(&facts)?.len() > request.max_bytes {
        return Err("conversation completion page exceeds byte budget".into());
    }
    Ok(facts)
}
