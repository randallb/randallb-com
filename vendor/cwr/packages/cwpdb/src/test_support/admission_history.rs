//! Fixed admission/history layout fixtures and finite observations.
use crate::CwpdbResult;
use rusqlite::Connection;
use serde_json::Value;
use std::path::Path;
pub fn assert_source_fingerprint_layout(path: &Path, fingerprint: &str) -> CwpdbResult<()> {
    let db = Connection::open(path)?;
    let properties: String = db.query_row(
        "SELECT json(props) FROM nodes WHERE kind='admission'",
        [],
        |r| r.get(0),
    )?;
    let properties: Value = serde_json::from_str(&properties)?;
    assert_eq!(properties["source_input_fingerprint"], fingerprint);
    assert!(properties["record"]
        .get("source_input_fingerprint")
        .is_none());
    Ok(())
}
pub fn seed_embedded_source_fingerprint(path: &Path, fingerprint: &str) -> CwpdbResult<()> {
    Connection::open(path)?.execute("UPDATE nodes SET props=jsonb_set(jsonb_remove(props,'$.source_input_fingerprint'),'$.record.source_input_fingerprint',?1) WHERE kind='admission'",[fingerprint])?;
    Ok(())
}
pub fn corrupt_cold_message_text_type(path: &Path, message: &str) -> CwpdbResult<()> {
    assert_eq!(Connection::open(path)?.execute("UPDATE nodes SET props=jsonb_set(props,'$.record.text',jsonb('42')) WHERE kind='message' AND ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[message])?,1);
    Ok(())
}
#[derive(Clone, Copy)]
pub enum ColdHeaderRelationship {
    Participant,
    RootConversation,
}
pub fn corrupt_cold_header_position(
    path: &Path,
    relationship: ColdHeaderRelationship,
) -> CwpdbResult<()> {
    let kind = match relationship {
        ColdHeaderRelationship::Participant => "participant",
        ColdHeaderRelationship::RootConversation => "root_conversation",
    };
    assert!(
        Connection::open(path)?.execute("UPDATE edges SET position=-1 WHERE kind=?1", [kind])? > 0
    );
    Ok(())
}
pub fn corrupt_runtime_root_target_kind(path: &Path) -> CwpdbResult<()> {
    assert_eq!(Connection::open(path)?.execute("UPDATE edges SET target_id=(SELECT ref_id FROM nodes WHERE kind='message' LIMIT 1) WHERE kind='root_conversation'",[])?,1);
    Ok(())
}
pub fn seed_unowned_custom_conversation(path: &Path) -> CwpdbResult<()> {
    let mut db = Connection::open(path)?;
    let tx = db.transaction()?;
    let custom = "cwp://example.test/custom/conversation";
    let parent = "cwp://example.test/custom/parent";
    let actor = "cwp://example.test/custom/actor";
    crate::put_node(
        &tx,
        custom,
        "conversation",
        &serde_json::json!({"custom":true}),
    )?;
    crate::put_node(
        &tx,
        parent,
        "conversation",
        &serde_json::json!({"custom":true}),
    )?;
    crate::put_node(&tx, actor, "actor", &serde_json::json!({}))?;
    crate::put_edge(&tx, custom, "parent", parent, 0, &serde_json::json!({}))?;
    crate::put_edge(&tx, custom, "participant", actor, 0, &serde_json::json!({}))?;
    tx.commit()?;
    Ok(())
}
