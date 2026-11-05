//! Fixed legacy relationship/ownership fixtures. Physical setup and snapshots
//! stay in cwpdb; downstream tests apply their pure declaration rules to facts.
use crate::{CwpdbResult, NativeStore};
use rusqlite::Connection;
use serde_json::{json, Value};

#[derive(Clone, Copy)]
pub enum RetainedCase {
    MissingMessage,
    ForwardMessage,
    BindingParticipant,
    ParticipantWrongKind,
    ParticipantPerson,
    MessageWrongKind,
    ProjectionEdge,
    MissingProjectionEdge,
    HistoricalBinding,
    CanonicalInvocationMissing,
    HistoricalWrongKind,
    UnownedMessage,
    OwnedInvalidMessage,
    OwnedInvalidActivity,
    WrongActivitySchema,
    UnownedKinds,
    OwnedOrphan,
}
#[derive(Debug)]
pub struct NodeFact {
    pub reference: String,
    pub kind: String,
    pub schema: String,
    pub value: Value,
}
#[derive(Debug)]
pub struct RelationshipFact {
    pub source: String,
    pub kind: String,
    pub target: String,
    pub position: i64,
    pub value: Value,
    pub schema: String,
    pub source_node: Option<String>,
    pub target_node: Option<String>,
    pub source_edge: Option<String>,
    pub target_edge: Option<String>,
}
#[derive(Debug, Default)]
pub struct RetainedFacts {
    pub records: Vec<NodeFact>,
    pub relationships: Vec<RelationshipFact>,
}
const R: &str = "cwp://example.test/runtime";
const C: &str = "cwp://example.test/conversation";
const M: &str = "cwp://example.test/message";
const A: &str = "cwp://example.test/actor";
const I: &str = "cwp://example.test/history/invocation";
const ACT: &str = "cwp://example.test/activity";
/// Each variant seeds one fixed fictional retained layout and returns its
/// ownership-limited facts from the same native transaction. No callbacks,
/// SQL strings or native handles cross the fixture boundary.
pub fn retained_case(case: RetainedCase) -> CwpdbResult<RetainedFacts> {
    let mut store = NativeStore::open_in_memory()?;
    store.graph_transaction(|tx| {
        let put = |r: &str, k: &str| crate::put_node(tx, r, k, &json!({}));
        let edge = |s: &str, k: &str, t: &str| crate::put_edge(tx, s, k, t, 0, &json!({}));
        use RetainedCase::*;
        match case {
            UnownedMessage | OwnedInvalidMessage => {
                put(M, "message")?;
                if matches!(case, OwnedInvalidMessage) {
                    crate::claim_identity(tx, C, "node:conversation", C)?;
                    put(C, "conversation")?;
                    edge(C, "message", M)?;
                }
            }
            UnownedKinds | OwnedOrphan => {
                for k in [
                    "conversation",
                    "activity",
                    "message",
                    "conversation_transfer_metadata",
                    "conversation_timeline_connection",
                    "conversation_timeline_item",
                ] {
                    crate::put_node(
                        tx,
                        &format!("cwp://example.test/custom/{k}"),
                        k,
                        &json!({"title":[],"record":false}),
                    )?;
                }
                if matches!(case, OwnedOrphan) {
                    crate::claim_identity(tx, C, "node:conversation", C)?;
                    put(C, "conversation")?;
                    let orphan = "cwp://example.test/timelines/owned-orphan";
                    crate::claim_identity(tx, orphan, "node:conversation_timeline_connection", C)?;
                    put(orphan, "conversation_timeline_connection")?;
                }
            }
            OwnedInvalidActivity | WrongActivitySchema => {
                put(R, "runtime")?;
                put(C, "conversation")?;
                edge(R, "conversation", C)?;
                let schema = if matches!(case, WrongActivitySchema) {
                    "cwp://example.test/schemas/custom".into()
                } else {
                    crate::schema_ref("activity")
                };
                crate::put_node_with_schema(tx, ACT, "activity", &schema, &json!({}))?;
                edge(C, "activity", ACT)?;
            }
            _ => {
                put(R, "runtime")?;
                put(C, "conversation")?;
                edge(R, "conversation", C)?;
                if !matches!(
                    case,
                    HistoricalBinding | CanonicalInvocationMissing | HistoricalWrongKind
                ) {
                    edge(C, "message", M)?;
                    if !matches!(case, MissingMessage) {
                        put(
                            M,
                            if matches!(case, MessageWrongKind) {
                                "invocation"
                            } else {
                                "message"
                            },
                        )?;
                    }
                }
                if matches!(
                    case,
                    BindingParticipant
                        | ParticipantWrongKind
                        | ParticipantPerson
                        | MessageWrongKind
                        | ProjectionEdge
                        | MissingProjectionEdge
                ) {
                    edge(C, "participant", A)?;
                    if matches!(case, ParticipantWrongKind) {
                        put(A, "message")?;
                    }
                    if matches!(case, ParticipantPerson) {
                        put(A, "person")?;
                    }
                }
                if matches!(
                    case,
                    ProjectionEdge
                        | MissingProjectionEdge
                        | HistoricalBinding
                        | CanonicalInvocationMissing
                        | HistoricalWrongKind
                ) {
                    put(ACT, "activity")?;
                    edge(C, "timeline_activity", ACT)?;
                    let placement = crate::derived_ref("timeline_activity", &[C, ACT]);
                    edge(C, "timeline_item", &placement)?;
                    if matches!(case, MissingProjectionEdge) {
                        edge(M, "timeline_source", &placement)?;
                        tx.execute("DELETE FROM edges WHERE kind='timeline_activity'", [])?;
                    }
                    if matches!(
                        case,
                        HistoricalBinding | CanonicalInvocationMissing | HistoricalWrongKind
                    ) {
                        edge(I, "timeline_invocation", &placement)?;
                        edge(I, "timeline_parent", &placement)?;
                        if matches!(case, HistoricalWrongKind) {
                            put(I, "message")?;
                        }
                        if matches!(case, CanonicalInvocationMissing) {
                            let item = "cwp://example.test/item";
                            put(item, "conversation_timeline_item")?;
                            edge(
                                C,
                                "conversation_timeline_root",
                                "cwp://example.test/connection",
                            )?;
                            put(
                                "cwp://example.test/connection",
                                "conversation_timeline_connection",
                            )?;
                            edge(
                                "cwp://example.test/connection",
                                "conversation_timeline_item",
                                item,
                            )?;
                            edge(item, "conversation_timeline_invocation", I)?;
                        }
                    }
                }
            }
        }
        snapshot(tx)
    })
}
fn snapshot(db: &Connection) -> CwpdbResult<RetainedFacts> {
    let mut facts = RetainedFacts::default();
    let sql=format!("{RETAINED_CONVERSATION_OWNERSHIP} SELECT n.kind,r.canonical_ref,s.canonical_ref,json(n.props) FROM owned_records owned JOIN nodes n ON n.ref_id=owned.ref_id AND n.kind=owned.kind JOIN ref_bindings r ON r.ref_id=n.ref_id JOIN ref_bindings s ON s.ref_id=n.schema_id");
    let mut q = db.prepare(&sql)?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let value: String = r.get(3)?;
        facts.records.push(NodeFact {
            kind: r.get(0)?,
            reference: r.get(1)?,
            schema: r.get(2)?,
            value: serde_json::from_str(&value)?,
        });
    }
    let sql=format!("{RETAINED_CONVERSATION_OWNERSHIP} SELECT source.canonical_ref,e.kind,target.canonical_ref,e.position,json(e.props),schema.canonical_ref,sn.kind,tn.kind,se.kind,te.kind FROM edges e JOIN ref_bindings source ON source.ref_id=e.source_id JOIN ref_bindings target ON target.ref_id=e.target_id JOIN ref_bindings schema ON schema.ref_id=e.schema_id LEFT JOIN nodes sn ON sn.ref_id=e.source_id LEFT JOIN nodes tn ON tn.ref_id=e.target_id LEFT JOIN edges se ON se.ref_id=e.source_id LEFT JOIN edges te ON te.ref_id=e.target_id WHERE e.source_id IN (SELECT ref_id FROM owned_records) OR (e.kind IN ('conversation','root_conversation') AND e.source_id IN (SELECT ref_id FROM nodes WHERE kind='runtime')) OR e.kind IN ('timeline_invocation','timeline_parent')");
    let mut q = db.prepare(&sql)?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let value: String = r.get(4)?;
        facts.relationships.push(RelationshipFact {
            source: r.get(0)?,
            kind: r.get(1)?,
            target: r.get(2)?,
            position: r.get(3)?,
            value: serde_json::from_str(&value)?,
            schema: r.get(5)?,
            source_node: r.get(6)?,
            target_node: r.get(7)?,
            source_edge: r.get(8)?,
            target_edge: r.get(9)?,
        });
    }
    Ok(facts)
}

// Physical kind/schema names are reusable by independent entity declarations.
// Membership and domain owner claims identify the records CWR must validate.
// Keep legacy membership: owner claims are repaired only after state hydration.
const RETAINED_CONVERSATION_OWNERSHIP: &str = r#"
WITH RECURSIVE
owned_conversations(ref_id) AS (
    SELECT membership.target_id FROM edges membership
    JOIN nodes runtime ON runtime.ref_id=membership.source_id AND runtime.kind='runtime'
    WHERE membership.kind='conversation'
    UNION
    SELECT ref_id FROM ref_bindings
    WHERE claim_kind='node:conversation' AND claim_owner_ref=canonical_ref
),
owned_connections(ref_id) AS (
    SELECT edge.target_id FROM edges edge
    JOIN owned_conversations conversation ON conversation.ref_id=edge.source_id
    WHERE edge.kind='conversation_timeline_root'
    UNION
    SELECT claim.ref_id FROM ref_bindings claim
    JOIN ref_bindings owner ON owner.canonical_ref=claim.claim_owner_ref
    JOIN owned_conversations conversation ON conversation.ref_id=owner.ref_id
    WHERE claim.claim_kind='node:conversation_timeline_connection'
    UNION
    SELECT child.target_id FROM owned_connections parent
    JOIN edges item ON item.source_id=parent.ref_id AND item.kind='conversation_timeline_item'
    JOIN edges child ON child.source_id=item.target_id AND child.kind='conversation_timeline_child'
),
owned_items(ref_id) AS (
    SELECT edge.target_id FROM edges edge
    JOIN owned_connections connection ON connection.ref_id=edge.source_id
    WHERE edge.kind='conversation_timeline_item'
    UNION
    SELECT claim.ref_id FROM ref_bindings claim
    JOIN ref_bindings owner ON owner.canonical_ref=claim.claim_owner_ref
    JOIN owned_conversations conversation ON conversation.ref_id=owner.ref_id
    WHERE claim.claim_kind='node:conversation_timeline_item'
),
owned_records(ref_id,kind) AS (
    SELECT ref_id,'conversation' FROM owned_conversations
    UNION SELECT ref_id,'conversation_timeline_connection' FROM owned_connections
    UNION SELECT ref_id,'conversation_timeline_item' FROM owned_items
    UNION
    SELECT edge.target_id,CASE edge.kind
        WHEN 'activity' THEN 'activity'
        WHEN 'transfer_metadata' THEN 'conversation_transfer_metadata'
        ELSE 'message' END
    FROM edges edge JOIN owned_conversations conversation ON conversation.ref_id=edge.source_id
    WHERE edge.kind IN ('message','timeline_message','activity','transfer_metadata')
    UNION
    SELECT edge.target_id,'message' FROM edges edge
    JOIN owned_items item ON item.ref_id=edge.source_id
    WHERE edge.kind='conversation_timeline_message'
    UNION
    -- Streaming projections are loaded before recovery even when not yet linked
    -- into the retained conversation. Preserve that existing ownership signal.
    SELECT ref_id,'message' FROM nodes
    WHERE kind='message' AND json_type(props,'$.streaming')='object'
)
"#;
