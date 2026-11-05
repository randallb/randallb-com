use super::*;
use sha2::{Digest, Sha256};
pub(in crate::conversation::native) fn flat_revision(
    db: &Connection,
    root: &str,
) -> CwpdbResult<String> {
    let mut statement=db.prepare_cached("WITH RECURSIVE reachable(id) AS (SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1 UNION SELECT child.target_id FROM reachable parent JOIN edges item ON item.source_id=parent.id AND item.kind='conversation_timeline_item' JOIN edges child ON child.source_id=item.target_id AND child.kind='conversation_timeline_child' LIMIT 50001) SELECT reference.canonical_ref,json_extract(node.props,'$.revision'),schema.canonical_ref,json_extract(node.props,'$.store_sequence'),node.kind FROM reachable LEFT JOIN nodes node ON node.ref_id=reachable.id JOIN ref_bindings reference ON reference.ref_id=reachable.id LEFT JOIN ref_bindings schema ON schema.ref_id=node.schema_id ORDER BY json_extract(node.props,'$.store_sequence'),node.ref_id")?;
    let rows = statement
        .query_map([root], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() > 50000 {
        return Err("conversation visibility work limit exceeded".into());
    }
    if rows.is_empty() {
        return Err("conversation timeline root is unavailable".into());
    }
    if rows[0].0 != root {
        return Err("timeline connection order requires migration".into());
    }
    let mut digest = Sha256::new();
    digest.update(b"cwr-canonical-flat-timeline-v1\0");
    let mut sequences = std::collections::BTreeSet::new();
    for (reference, revision, schema, sequence, kind) in rows {
        if kind != "conversation_timeline_connection" {
            return Err("timeline connection has invalid kind".into());
        }
        let sequence = sequence
            .filter(|value| *value >= 0)
            .ok_or("timeline connection order requires migration")?;
        if !sequences.insert(sequence) {
            return Err("duplicate timeline connection order requires migration".into());
        }
        if schema != crate::schema_ref("conversation_timeline_connection") {
            return Err("unsupported timeline connection schema".into());
        }
        digest.update(u64::try_from(reference.len())?.to_be_bytes());
        digest.update(reference.as_bytes());
        digest.update(u64::try_from(revision)?.to_be_bytes());
    }
    Ok(format!("canonical:v1:{}", hex::encode(digest.finalize())))
}
pub(in crate::conversation::native) const WALK:&str="WITH RECURSIVE walk(item_id,path,depth) AS (
 SELECT item.target_id,printf('%020d',item.position),0 FROM edges item WHERE item.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND item.kind='conversation_timeline_item'
 UNION ALL SELECT nested.target_id,walk.path||'/'||printf('%020d',nested.position),walk.depth+1 FROM walk JOIN edges child ON child.source_id=walk.item_id AND child.kind='conversation_timeline_child' JOIN edges nested ON nested.source_id=child.target_id AND nested.kind='conversation_timeline_item' WHERE walk.depth<128 LIMIT 50001)";
pub(in crate::conversation::native) fn validate_walk(
    db: &Connection,
    root: &str,
) -> CwpdbResult<()> {
    let sql = format!("{WALK} SELECT COUNT(*),MAX(depth) FROM walk WHERE ?1 IS NOT NULL");
    let (count, depth): (i64, Option<i64>) = db.query_row(&sql, params!["", root], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    if count > 50000 || depth.is_some_and(|depth| depth >= 128) {
        return Err("conversation content walk work limit exceeded".into());
    }
    Ok(())
}
pub(in crate::conversation::native) fn message_relation(canonical: bool) -> String {
    let prefix = if canonical {
        format!("{WALK}, flat(position,item_id) AS (SELECT ROW_NUMBER() OVER (ORDER BY path)-1,item_id FROM walk WHERE NOT EXISTS(SELECT 1 FROM edges source JOIN nodes body ON body.ref_id=source.target_id WHERE source.source_id=walk.item_id AND source.kind='conversation_timeline_message' AND json_type(body.props,'$.streaming')='object')),first_placements(message_id,position) AS (SELECT content.target_id,MIN(flat.position) FROM flat JOIN edges content ON content.source_id=flat.item_id AND content.kind='conversation_timeline_message' GROUP BY content.target_id),")
    } else {
        "WITH ".into()
    };
    let source = if canonical {
        "first_placements position JOIN edges membership ON membership.target_id=position.message_id AND membership.kind='message'"
    } else {
        "edges membership"
    };
    let position = if canonical {
        "position.position"
    } else {
        "membership.position"
    };
    format!("{prefix} messages(position,message_ref,record,schema_ref) AS (SELECT {position},reference.canonical_ref,CASE WHEN json_type(membership.props,'$.visualizations') IS NULL THEN json_extract(node.props,'$.record') ELSE json_set(json_extract(node.props,'$.record'),'$.visualizations',json_extract(membership.props,'$.visualizations')) END,schema.canonical_ref FROM {source} JOIN nodes node ON node.ref_id=membership.target_id AND node.kind='message' JOIN ref_bindings reference ON reference.ref_id=node.ref_id JOIN ref_bindings schema ON schema.ref_id=node.schema_id WHERE membership.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND membership.kind='message' AND json_type(node.props,'$.record.progress') IS NULL AND (?2 IS NOT NULL))")
}
