use super::*;
use rusqlite::{params, OptionalExtension};
const LIMIT: usize = 50000;
const WALK: &str = "WITH RECURSIVE walk(item_id,path,depth) AS (
 SELECT item.target_id,printf('%020d',item.position),0 FROM edges item
 WHERE item.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND item.kind='conversation_timeline_item'
 UNION ALL SELECT nested.target_id,walk.path||'/'||printf('%020d',nested.position),walk.depth+1 FROM walk
 JOIN edges child ON child.source_id=walk.item_id AND child.kind='conversation_timeline_child'
 JOIN edges nested ON nested.source_id=child.target_id AND nested.kind='conversation_timeline_item'
 WHERE walk.depth<128 LIMIT 50001)";
const REACHABLE: &str = "WITH RECURSIVE connections(id) AS (
 SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2
 UNION SELECT child.target_id FROM connections parent
 JOIN edges item ON item.source_id=parent.id AND item.kind='conversation_timeline_item'
 JOIN edges child ON child.source_id=item.target_id AND child.kind='conversation_timeline_child'
 LIMIT 50001)";
struct Edge {
    target: String,
}
fn edges(db: &Connection, source: &str, kind: &str, limit: usize) -> CwpdbResult<Vec<Edge>> {
    let mut query = db.prepare_cached("SELECT target.canonical_ref FROM edges e INDEXED BY edges_out JOIN ref_bindings target ON target.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind=?2 ORDER BY e.position,e.ref_id LIMIT ?3")?;
    let result = query
        .query_map(params![source, kind, (limit + 1) as i64], |row| {
            Ok(Edge {
                target: row.get(0)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if result.len() > limit {
        return Err("conversation summary relationship work limit exceeded".into());
    }
    Ok(result)
}
fn one(db: &Connection, source: &str, kind: &str) -> CwpdbResult<Option<Edge>> {
    Ok(edges(db, source, kind, 1)?.pop())
}
fn latest(db: &Connection, conversation: &str, root: &str) -> CwpdbResult<Option<(String, Value)>> {
    let (count, depth): (i64, Option<i64>) = db.query_row(
        &format!("{WALK} SELECT COUNT(*),MAX(depth) FROM walk"),
        [root],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if count > LIMIT as i64 || depth.is_some_and(|depth| depth >= 128) {
        return Err("conversation summary timeline work limit exceeded".into());
    }
    let result:Option<(String,String,String)>=db.query_row(&format!("{WALK}
 SELECT reference.canonical_ref,json_extract(node.props,'$.record'),json(membership.props)
 FROM walk JOIN edges content ON content.source_id=walk.item_id AND content.kind='conversation_timeline_message'
 JOIN nodes node ON node.ref_id=content.target_id AND node.kind='message'
 JOIN ref_bindings reference ON reference.ref_id=node.ref_id
 JOIN edges membership INDEXED BY edges_in ON membership.target_id=node.ref_id AND membership.kind='message'
 AND membership.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)
 WHERE NOT EXISTS(SELECT 1 FROM edges source JOIN nodes streaming ON streaming.ref_id=source.target_id WHERE source.source_id=walk.item_id AND source.kind='conversation_timeline_message' AND json_type(streaming.props,'$.streaming')='object') ORDER BY walk.path DESC LIMIT 1"),params![root,conversation],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    result
        .map(|(reference, record, props)| {
            let mut record: Value = serde_json::from_str(&record)?;
            if let Some(visualizations) =
                serde_json::from_str::<Value>(&props)?.get("visualizations")
            {
                record
                    .as_object_mut()
                    .ok_or("invalid canonical message record")?
                    .insert("visualizations".into(), visualizations.clone());
            }
            Ok((reference, record))
        })
        .transpose()
}
fn completions(
    db: &Connection,
    conversation: &str,
    root: Option<&str>,
) -> CwpdbResult<Vec<SummaryCompletion>> {
    let sql = if root.is_some() {
        format!("{REACHABLE} SELECT r.canonical_ref,e.position FROM edges e INDEXED BY edges_out JOIN ref_bindings r ON r.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='completed_invocation' AND e.target_id IN (SELECT content.target_id FROM connections c JOIN edges item ON item.source_id=c.id AND item.kind='conversation_timeline_item' JOIN edges content ON content.source_id=item.target_id AND content.kind='conversation_timeline_invocation') ORDER BY e.position DESC,e.ref_id DESC LIMIT 500")
    } else {
        "SELECT r.canonical_ref,e.position FROM edges e INDEXED BY edges_out JOIN ref_bindings r ON r.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='completed_invocation' ORDER BY e.position DESC,e.ref_id DESC LIMIT 1".into()
    };
    let mut query = db.prepare(&sql)?;
    let decode = |row: &rusqlite::Row<'_>| {
        Ok(SummaryCompletion {
            invocation_ref: row.get(0)?,
            sequence: row.get(1)?,
        })
    };
    let mut result = if let Some(root) = root {
        query
            .query_map(params![conversation, root], decode)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        query
            .query_map([conversation], decode)?
            .collect::<Result<Vec<_>, _>>()?
    };
    result.reverse();
    Ok(result)
}
impl NativeConversationStore {
    pub(super) fn summary_facts(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<SummaryFacts>> {
        self.check_scope(scope)?;
        valid_ref(reference)?;
        let tx = self.connection.transaction()?;
        authorizer.authorize(&scope.context, reference, false)?;
        let Some(record) =
            read::get(&tx, reference)?.filter(|record| record.kind == RecordKind::Conversation)
        else {
            return Ok(None);
        };
        let root = one(&tx, reference, "conversation_timeline_root")?;
        if let Some(root) = &root {
            if read::get(&tx, &root.target)?
                .is_none_or(|record| record.kind != RecordKind::ConversationTimelineConnection)
            {
                return Err("conversation timeline root is unavailable".into());
            }
        }
        let root_membership:Option<i64>=tx.query_row("SELECT position FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='root_conversation' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)",params![root_ref(scope),reference],|row|row.get(0)).optional()?;
        let parent = one(&tx, reference, "parent")?.map(|edge| edge.target);
        let participants = edges(&tx, reference, "participant", LIMIT)?
            .into_iter()
            .map(|edge| edge.target)
            .collect();
        let archived = !edges(&tx, reference, "archived", 1)?.is_empty();
        let visible = if let Some(root) = &root {
            latest(&tx, reference, &root.target)?
        } else {
            None
        };
        let completed = completions(
            &tx,
            reference,
            root.as_ref().map(|edge| edge.target.as_str()),
        )?;
        let completed_work_refs = if root.is_some() {
            completed
                .iter()
                .map(|work| work.invocation_ref.clone())
                .collect()
        } else {
            serde_json::from_value(
                record
                    .properties
                    .get("completed_work_refs")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )?
        };
        let last_message_ref = if root.is_some() {
            visible.as_ref().map(|(reference, _)| reference.clone())
        } else {
            record.properties["last_message_ref"]
                .as_str()
                .map(str::to_owned)
        };
        let last_message_preview = if root.is_some() {
            visible.as_ref().map(|(_, record)| {
                record["text"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(240)
                    .collect()
            })
        } else {
            record.properties["last_message_preview"]
                .as_str()
                .map(str::to_owned)
        };
        let facts = SummaryFacts {
            conversation_ref: reference.into(),
            creation_order: root_membership
                .or_else(|| record.properties["creation_order"].as_i64())
                .unwrap_or(0),
            title: record.properties["title"].as_str().map(str::to_owned),
            archived,
            owner_context_ref: record.properties["owner_context_ref"]
                .as_str()
                .map(str::to_owned),
            parent_ref: parent,
            sidebar_eligible: record.properties["sidebar_eligible"]
                .as_bool()
                .unwrap_or(true),
            root: root_membership.is_some(),
            participant_actor_refs: participants,
            last_message_ref,
            last_message_preview,
            last_message_record: visible.map(|(_, record)| record),
            completed_work: completed.last().cloned().into_iter().collect(),
            completed_work_refs,
            revision: String::new(),
        };
        let result = super::super::summary::finish(facts)?;
        authorizer.authorize(&scope.context, reference, false)?;
        tx.commit()?;
        Ok(Some(result))
    }
}
