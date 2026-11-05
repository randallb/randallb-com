//! Named retained-timeline corruption and observation fixtures. No physical handle leaves cwpdb.
use crate::CwpdbResult;
use rusqlite::{params, Connection};
use serde_json::json;
use std::path::Path;
fn open(path: &Path) -> CwpdbResult<Connection> {
    Ok(Connection::open(path)?)
}
pub fn commit_counter(path: &Path) -> CwpdbResult<i64> {
    Ok(open(path)?.query_row(
        "SELECT json_extract(props,'$.commit_revision') FROM nodes WHERE kind='runtime'",
        [],
        |r| r.get(0),
    )?)
}
pub fn state_version(path: &Path) -> CwpdbResult<String> {
    Ok(open(path)?.query_row(
        "SELECT json_extract(props,'$.runtime_state_version') FROM nodes WHERE kind='runtime'",
        [],
        |r| r.get(0),
    )?)
}
pub fn erase_explicit_timeline_writer_fence(path: &Path) -> CwpdbResult<()> {
    open(path)?.execute("UPDATE nodes SET props=jsonb_set(props,'$.runtime_state_version','cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09') WHERE kind='runtime'",[])?;
    Ok(())
}
pub fn seed_message_identity_collision(path: &Path, reference: &str) -> CwpdbResult<()> {
    let mut db = open(path)?;
    let tx = db.transaction()?;
    crate::put_node(&tx, reference, "message", &json!({}))?;
    tx.commit()?;
    Ok(())
}
pub fn seed_timeline_root_edge_collision(
    path: &Path,
    first: &str,
    second: &str,
) -> CwpdbResult<String> {
    let mut db = open(path)?;
    let tx = db.transaction()?;
    crate::put_edge(&tx, first, "timeline-root-collision", second, 0, &json!({}))?;
    tx.commit()?;
    Ok(crate::derived_ref(
        "timeline-root-collision",
        &[first, second],
    ))
}
pub fn seed_canonical_completion_history(
    path: &Path,
    root: &str,
    conversation: &str,
    timeline: &str,
) -> CwpdbResult<()> {
    let mut db = open(path)?;
    let tx = db.transaction()?;
    for sequence in 1..=501_i64 {
        let invocation = format!("{root}/invocations/canonical-history-{sequence}");
        let item = format!("{timeline}/items/{sequence}");
        crate::put_node(
            &tx,
            &item,
            "conversation_timeline_item",
            &json!({"publication_ref":format!("{conversation}/publications/{sequence}")}),
        )?;
        crate::put_edge(
            &tx,
            timeline,
            "conversation_timeline_item",
            &item,
            sequence,
            &json!({}),
        )?;
        crate::put_edge(
            &tx,
            &item,
            "conversation_timeline_invocation",
            &invocation,
            0,
            &json!({}),
        )?;
        crate::put_edge(
            &tx,
            conversation,
            "completed_invocation",
            &invocation,
            sequence,
            &json!({}),
        )?;
    }
    tx.commit()?;
    Ok(())
}
pub fn seed_legacy_completion(path: &Path, conversation: &str, sequence: i64) -> CwpdbResult<()> {
    let db = open(path)?;
    let invocation = format!("cwp://com.example.runtime/invocations/history-{sequence}");
    let edge = format!("cwp://com.example.runtime/edges/history-{sequence}");
    db.execute(
        "INSERT INTO ref_bindings(canonical_ref) VALUES(?1),(?2)",
        params![invocation, edge],
    )?;
    db.execute("INSERT INTO edges(ref_id,kind,schema_id,source_id,target_id,position,props) SELECT (SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1),'completed_invocation',schema_id,(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2),(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3),?4,jsonb('{}') FROM nodes LIMIT 1",params![edge,conversation,invocation,sequence])?;
    Ok(())
}
pub fn assert_backward_readable_fork(path: &Path) -> CwpdbResult<()> {
    let db = open(path)?;
    let incompatible:i64=db.query_row("SELECT COUNT(*) FROM nodes WHERE kind='activity' AND json_extract(props,'$.record.event.kind') IN ('conversation_forked_from','message_work_association')",[],|r|r.get(0))?;
    assert_eq!(incompatible, 0);
    let timeline:i64=db.query_row("SELECT COUNT(*) FROM edges WHERE kind='timeline_activity' AND json_extract(props,'$.record.event.kind') IN ('conversation_forked_from','message_work_association')",[],|r|r.get(0))?;
    assert_eq!(timeline, 0);
    let metadata: i64 = db.query_row(
        "SELECT COUNT(*) FROM nodes WHERE kind='conversation_transfer_metadata'",
        [],
        |r| r.get(0),
    )?;
    assert!(metadata > 0);
    Ok(())
}
pub fn seed_unsupported_user_version(path: &Path) -> CwpdbResult<()> {
    let db = open(path)?;
    db.pragma_update(None, "journal_mode", "DELETE")?;
    db.pragma_update(None, "user_version", 99)?;
    Ok(())
}
pub fn assert_flat_retained_layout(
    path: &Path,
    conversation: &str,
    invocation: &str,
    publication: &str,
) -> CwpdbResult<()> {
    let db = open(path)?;
    let tables: i64 = db.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(tables, 3, "the timeline stays in the shared CWPDB graph");
    let embedded:Option<String>=db.query_row("SELECT json_type(props,'$.timeline') FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[conversation],|r|r.get(0))?;
    assert_eq!(embedded, None);
    let links:i64=db.query_row("SELECT COUNT(*) FROM edges WHERE kind='conversation_timeline_invocation' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[invocation],|r|r.get(0))?;
    assert_eq!(links, 2);
    assert_eq!(
        crate::identity_claim(&db, publication)?,
        Some(crate::IdentityClaim {
            kind: "conversation_timeline_publication".into(),
            owner_ref: Some(conversation.into())
        })
    );
    Ok(())
}
pub fn publication_claim(
    path: &Path,
    publication: &str,
) -> CwpdbResult<Option<crate::IdentityClaim>> {
    crate::identity_claim(&open(path)?, publication)
}
pub struct ProjectionFacts {
    pub commit: i64,
    pub projection_version: i64,
    pub source_items: i64,
}
pub fn erase_old_timeline_projection(path: &Path, message: &str) -> CwpdbResult<i64> {
    let db = open(path)?;
    db.execute("DELETE FROM edges WHERE kind='timeline_item' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[message])?;
    db.execute("UPDATE nodes SET props=jsonb_remove(props,'$.timeline_projection_version') WHERE kind='runtime'",[])?;
    commit_counter(path)
}
pub fn projection_facts(path: &Path, message: &str) -> CwpdbResult<ProjectionFacts> {
    let db = open(path)?;
    Ok(ProjectionFacts{commit:commit_counter(path)?,projection_version:db.query_row("SELECT json_extract(props,'$.timeline_projection_version') FROM nodes WHERE kind='runtime'",[],|r|r.get(0))?,source_items:db.query_row("SELECT COUNT(*) FROM edges WHERE kind='timeline_item' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[message],|r|r.get(0))?})
}
pub struct BindingSnapshot(Vec<(i64, String)>);
fn bindings(db: &Connection) -> CwpdbResult<Vec<(i64, String)>> {
    Ok(db
        .prepare("SELECT ref_id,canonical_ref FROM ref_bindings ORDER BY ref_id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?)
}
pub fn seed_unknown_properties(path: &Path) -> CwpdbResult<BindingSnapshot> {
    let db = open(path)?;
    db.execute("UPDATE nodes SET props=jsonb_set(props,'$.extension',jsonb('{\"future\":true}')) WHERE kind='conversation'",[])?;
    db.execute("UPDATE edges SET props=jsonb_set(props,'$.extension',jsonb('{\"future\":true}')) WHERE kind='timeline_message'",[])?;
    Ok(BindingSnapshot(bindings(&db)?))
}
pub fn assert_bindings_preserved(path: &Path, expected: &BindingSnapshot) -> CwpdbResult<()> {
    let current = bindings(&open(path)?)?;
    for binding in &expected.0 {
        assert!(current.contains(binding));
    }
    Ok(())
}
pub fn assert_unknown_properties_preserved(path: &Path) -> CwpdbResult<()> {
    let db = open(path)?;
    let conversations:i64=db.query_row("SELECT COUNT(*) FROM nodes WHERE kind='conversation' AND json_extract(props,'$.extension.future')=1",[],|r|r.get(0))?;
    assert_eq!(conversations, 2);
    let messages:i64=db.query_row("SELECT COUNT(*) FROM edges WHERE kind='timeline_message' AND json_extract(props,'$.extension.future')=1",[],|r|r.get(0))?;
    assert!(messages > 0);
    Ok(())
}
pub struct VisualizationFacts {
    pub message_record: String,
    pub preview: String,
    pub visualization_count: i64,
}
pub fn visualization_facts(
    path: &Path,
    conversation: &str,
    message: &str,
) -> CwpdbResult<VisualizationFacts> {
    let db = open(path)?;
    Ok(VisualizationFacts{message_record:db.query_row("SELECT json_extract(props,'$.record') FROM nodes WHERE kind='message' AND json_extract(props,'$.record.message_ref')=?1",[message],|r|r.get(0))?,preview:db.query_row("SELECT json_extract(props,'$.last_message_preview') FROM nodes WHERE kind='conversation' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[conversation],|r|r.get(0))?,visualization_count:db.query_row("SELECT json_array_length(json_extract(props,'$.visualizations')) FROM edges WHERE kind='message' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[message],|r|r.get::<_,Option<i64>>(0))?.unwrap_or(0)})
}
pub fn seed_legacy_visualization_metadata(
    path: &Path,
    conversation: &str,
    message: &str,
    visualizations: &serde_json::Value,
) -> CwpdbResult<i64> {
    let db = open(path)?;
    db.execute("UPDATE nodes SET props=jsonb_set(props,'$.record.visualizations',json(?2)) WHERE kind='message' AND json_extract(props,'$.record.message_ref')=?1",params![message,visualizations.to_string()])?;
    db.execute("UPDATE edges SET props=jsonb_remove(props,'$.visualizations') WHERE kind='message' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[message])?;
    db.execute("UPDATE nodes SET props=jsonb_set(props,'$.last_message_preview',?2) WHERE kind='conversation' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",params![conversation,"/private/chart.html"])?;
    commit_counter(path)
}
pub fn corrupt_visualization_with_oversized_html(path: &Path, bytes: usize) -> CwpdbResult<()> {
    open(path)?.execute("UPDATE nodes SET props=jsonb_set(props,'$.record.html',?1) WHERE kind='visualization_artifact'",["x".repeat(bytes)])?;
    Ok(())
}
pub fn corrupt_visualization_artifact_reference(path: &Path) -> CwpdbResult<()> {
    open(path)?.execute("UPDATE nodes SET props=jsonb_set(props,'$.record.html','ok','$.record.artifact_ref','cwp://example.test/wrong') WHERE kind='visualization_artifact'",[])?;
    Ok(())
}
pub fn seed_future_invocation_schema(path: &Path) -> CwpdbResult<()> {
    let mut db = open(path)?;
    let tx = db.transaction()?;
    let future = crate::bind(&tx, "cwp://example/future-invocation-schema")?;
    tx.execute(
        "UPDATE nodes SET schema_id=?1 WHERE kind='invocation'",
        [future],
    )?;
    tx.commit()?;
    Ok(())
}
pub fn assert_future_invocation_schema_preserved(path: &Path) -> CwpdbResult<()> {
    let db = open(path)?;
    let schema:String=db.query_row("SELECT r.canonical_ref FROM nodes n JOIN ref_bindings r ON r.ref_id=n.schema_id WHERE n.kind='invocation'",[],|r|r.get(0))?;
    assert_eq!(schema, "cwp://example/future-invocation-schema");
    Ok(())
}
