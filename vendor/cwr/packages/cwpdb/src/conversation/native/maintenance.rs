//! Read-only anomaly probes and layout repair. CWR supplies validated canonical
//! publication deltas; this module never reimplements authoritative projection.
use super::*;
use rusqlite::{params, Transaction};
fn owned_query(suffix: &str) -> String {
    format!("{} {suffix}", include_str!("owned.sql"))
}
fn exists(db: &Connection, sql: &str) -> CwpdbResult<bool> {
    Ok(db.query_row(sql, [], |row| row.get(0))?)
}
fn boundary_probe() -> String {
    owned_query("SELECT EXISTS(SELECT 1 FROM owned_records own JOIN nodes n ON n.ref_id=own.ref_id AND n.kind=own.kind WHERE (own.kind='activity' AND json_extract(n.props,'$.record.event.kind')='message_recorded' AND NOT EXISTS(SELECT 1 FROM edges link WHERE link.source_id=n.ref_id AND link.kind='field:event.message_ref' AND link.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=json_extract(n.props,'$.record.event.message_ref')))) OR (own.kind='message' AND json_type(n.props,'$.record.progress.source_activity_ref')='text' AND NOT EXISTS(SELECT 1 FROM edges link WHERE link.source_id=n.ref_id AND link.kind='field:progress.source_activity_ref' AND link.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=json_extract(n.props,'$.record.progress.source_activity_ref')))))")
}
fn visual_probe() -> String {
    owned_query("SELECT EXISTS(SELECT 1 FROM owned_records own JOIN nodes n ON n.ref_id=own.ref_id AND n.kind=own.kind WHERE own.kind='message' AND json_type(n.props,'$.record.visualizations') IS NOT NULL)")
}
fn claims(db: &Connection) -> CwpdbResult<Vec<(String, String, String)>> {
    Ok(db
        .prepare(include_str!("identity_claims.sql"))?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<_, _>>()?)
}
pub(super) fn needed(db: &Connection, scope: &Scope) -> CwpdbResult<Vec<&'static str>> {
    let mut needed = Vec::new();
    if exists(db, &boundary_probe())? {
        needed.push("message_boundary");
    }
    if exists(db, &visual_probe())? {
        needed.push("visualization_metadata");
    }
    if exists(db, include_str!("identity_claim_probe.sql"))? {
        needed.push("identity_owners");
    }
    let metadata = runtime_metadata(db, scope)?;
    let version = metadata["timeline_projection_version"]
        .as_i64()
        .unwrap_or(0);
    if version > 1 {
        return Err("unsupported timeline projection version".into());
    }
    if version < 1 {
        needed.push("timeline_projection");
    }
    let missing=exists(db,"SELECT EXISTS(SELECT 1 FROM nodes WHERE kind='invocation' AND json_extract(props,'$.record.state')='completed' AND json_extract(props,'$.record.parent_invocation_ref') IS NULL AND json_extract(props,'$.record.forked_from_invocation_ref') IS NULL AND json_type(props,'$.completion_order') IS NULL)")?;
    let imported=exists(db,&owned_query("SELECT EXISTS(SELECT 1 FROM owned_conversations own JOIN edges e ON e.source_id=own.ref_id AND e.kind='activity' JOIN nodes n ON n.ref_id=e.target_id WHERE json_extract(n.props,'$.record.event.kind')='work_lifecycle' AND json_extract(n.props,'$.record.event.state')='completed' AND json_extract(n.props,'$.record.event.parent_invocation_ref') IS NULL AND e.position>COALESCE((SELECT MAX(lineage.position) FROM edges lineage JOIN nodes transfer ON transfer.ref_id=lineage.target_id WHERE lineage.source_id=own.ref_id AND lineage.kind='transfer_metadata' AND json_extract(transfer.props,'$.record.event.kind')='conversation_forked_from'),-1) AND NOT EXISTS(SELECT 1 FROM edges done WHERE done.source_id=own.ref_id AND done.kind='completed_invocation' AND done.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=json_extract(n.props,'$.record.event.invocation_ref'))))"))?;
    if missing || imported {
        needed.push("completion_order");
    }
    Ok(needed)
}
pub(super) fn repair(tx: &Transaction<'_>, request: &Mutation, next: i64) -> CwpdbResult<bool> {
    let needs = needed(tx, &request.scope)?;
    if needs.is_empty() {
        return Ok(false);
    }
    if needs.contains(&"timeline_projection") {
        let roots = crate::targets(tx, &root_ref(&request.scope), "conversation")?;
        for conversation in roots {
            if crate::targets(tx, &conversation, "parent")?.is_empty()
                && !request
                    .changes
                    .publications
                    .iter()
                    .any(|p| p.conversation_ref == conversation && p.from_position == 0)
            {
                return Err(
                    "legacy timeline repair requires canonical root publication suffixes".into(),
                );
            }
        }
    }
    let records=tx.prepare(&owned_query("SELECT r.canonical_ref,n.kind,json(n.props) FROM owned_records own JOIN nodes n ON n.ref_id=own.ref_id AND n.kind=own.kind JOIN ref_bindings r ON r.ref_id=n.ref_id"))?.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
    for (reference, kind, props) in records {
        let properties: Value = serde_json::from_str(&props)?;
        if kind == "activity" && properties["record"]["event"]["kind"] == "message_recorded" {
            write::records::references(tx, &reference, &properties["record"])?;
        }
        if kind == "message" {
            if properties["record"]["progress"]["source_activity_ref"].is_string() {
                write::records::references(tx, &reference, &properties["record"])?;
            }
            if let Some(visualizations) = properties["record"].get("visualizations") {
                let parents=tx.prepare("SELECT r.canonical_ref,e.position,json(e.props) FROM edges e JOIN ref_bindings r ON r.ref_id=e.source_id WHERE e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='message'")?.query_map([&reference],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
                for (parent, position, props) in parents {
                    let mut props: Value = serde_json::from_str(&props)?;
                    props["visualizations"] = visualizations.clone();
                    crate::put_edge(tx, &parent, "message", &reference, position, &props)?;
                }
                tx.execute("UPDATE nodes SET props=jsonb_remove(props,'$.record.visualizations'),revision=revision+1 WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[&reference])?;
            }
        }
    }
    for (reference, kind, owner) in claims(tx)? {
        crate::claim_identity(tx, &reference, &kind, &owner)?;
    }
    let invocations=tx.prepare("SELECT r.canonical_ref,json(n.props) FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id LEFT JOIN edges admission ON admission.target_id=n.ref_id AND admission.kind='field:invocation_ref' LEFT JOIN edges membership ON membership.target_id=admission.source_id AND membership.kind='admission' WHERE n.kind='invocation' AND json_extract(n.props,'$.record.state')='completed' AND json_type(n.props,'$.completion_order') IS NULL ORDER BY membership.position,n.ref_id")?.query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    for (reference, props) in invocations {
        let props: Value = serde_json::from_str(&props)?;
        write::records::completion(tx, &request.scope, &reference, &props["record"])?;
    }
    let conversations = crate::targets(tx, &root_ref(&request.scope), "conversation")?;
    for conversation in conversations {
        write::memberships::root_projection(tx, &request.scope, &conversation)?;
        write::records::refresh_completions(tx, &request.scope, &conversation)?;
    }
    tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.timeline_projection_version',1,'$.commit_revision',?2) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",params![root_ref(&request.scope),next])?;
    Ok(true)
}
