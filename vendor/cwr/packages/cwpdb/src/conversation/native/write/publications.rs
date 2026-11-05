use super::*;
use rusqlite::OptionalExtension;
pub(super) fn apply(
    tx: &Transaction<'_>,
    publication: &PublicationSuffix,
    scope: &Scope,
    revision: i64,
) -> CwpdbResult<bool> {
    valid_ref(&publication.conversation_ref)?;
    let from = i64::try_from(publication.from_position)?;
    let old=tx.prepare("SELECT r.canonical_ref,json_extract(e.props,'$.item_kind') FROM edges e JOIN ref_bindings r ON r.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='timeline_item' AND e.position>=?2 ORDER BY e.position")?.query_map(params![publication.conversation_ref,from],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    let desired = publication
        .items
        .iter()
        .map(|item| match item {
            PublishedItem::Message { message_ref, .. } => (message_ref.as_str(), "message"),
            PublishedItem::Turn { invocation_ref, .. } => (invocation_ref.as_str(), "turn"),
        })
        .collect::<Vec<_>>();
    let mut unique = std::collections::BTreeSet::new();
    for (reference, _) in &desired {
        valid_ref(reference)?;
        if !unique.insert(*reference) {
            return Err("duplicate timeline publication".into());
        }
    }
    let mut changed = old
        .iter()
        .map(|(reference, kind)| (reference.as_str(), kind.as_str()))
        .ne(desired.iter().copied());
    let anchors_changed = changed;
    for (reference, kind) in &old {
        if !desired.iter().any(|(wanted, _)| *wanted == reference) {
            if kind == "message" {
                crate::remove_edge(
                    tx,
                    &publication.conversation_ref,
                    "timeline_message",
                    reference,
                )?;
            }
            crate::remove_edge(
                tx,
                &publication.conversation_ref,
                "timeline_item",
                reference,
            )?;
        }
    }
    for (offset, item) in publication.items.iter().enumerate() {
        let (reference, kind) = desired[offset];
        let position = from
            .checked_add(i64::try_from(offset)?)
            .ok_or("timeline position exhausted")?;
        crate::put_edge(
            tx,
            &publication.conversation_ref,
            "timeline_item",
            reference,
            position,
            &json!({"item_kind":kind}),
        )?;
        match item {
            PublishedItem::Message { position, .. } => {
                crate::put_edge(
                    tx,
                    &publication.conversation_ref,
                    "timeline_message",
                    reference,
                    i64::try_from(*position)?,
                    &json!({}),
                )?;
            }
            PublishedItem::Turn { .. } => {}
        }
    }
    for reference in &publication.remove_activities {
        crate::remove_edge(
            tx,
            &publication.conversation_ref,
            "timeline_activity",
            reference,
        )?;
        changed = true;
    }
    for activity in &publication.activities {
        let reference = &activity.activity_ref;
        valid_ref(reference)?;
        let position = i64::try_from(activity.position)?;
        let record = &activity.record;
        let props = json!({"record":record});
        let edge_ref = crate::derived_ref(
            "timeline_activity",
            &[&publication.conversation_ref, reference],
        );
        let previous=tx.query_row("SELECT json(props) FROM edges WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[&edge_ref],|row|row.get::<_,String>(0)).optional()?;
        if previous
            .map(|props| serde_json::from_str::<Value>(&props))
            .transpose()?
            .as_ref()
            != Some(&props)
        {
            changed = true;
        }
        crate::put_edge(
            tx,
            &publication.conversation_ref,
            "timeline_activity",
            reference,
            position,
            &props,
        )?;
        let work = matches!(
            record["event"]["kind"].as_str(),
            Some("work_update" | "work_lifecycle")
        );
        for (kind, field) in [
            ("timeline_invocation", "invocation_ref"),
            ("timeline_source", "source_message_ref"),
            ("timeline_parent", "parent_invocation_ref"),
        ] {
            let wanted = work.then(|| record["event"][field].as_str()).flatten();
            let previous=tx.prepare("SELECT r.canonical_ref FROM edges e JOIN ref_bindings r ON r.ref_id=e.source_id WHERE e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind=?2")?.query_map(params![edge_ref,kind],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            for source in previous {
                if Some(source.as_str()) != wanted {
                    crate::remove_edge(tx, &source, kind, &edge_ref)?;
                }
            }
            if let Some(source) = wanted {
                crate::put_edge(tx, source, kind, &edge_ref, position, &json!({}))?;
            }
        }
    }
    let completions_changed =
        super::records::refresh_completions(tx, scope, &publication.conversation_ref)?;
    changed |= completions_changed;
    if anchors_changed || completions_changed {
        tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.timeline_revision',?2) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",params![publication.conversation_ref,revision])?;
    }
    Ok(changed)
}
