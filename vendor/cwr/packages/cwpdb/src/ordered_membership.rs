//! Existing low-level ordered membership primitives; CWR owns declared operations.
use crate::CwpdbResult;
use rusqlite::{params, Connection, Transaction};

#[derive(Clone, Debug, PartialEq)]
pub struct OrderedMember {
    pub target: String,
    pub edge_ref: String,
    pub properties: serde_json::Value,
}
/// Bounded complete-list read; oversize scopes fail rather than truncate.
pub fn read_ordered_members(
    db: &Connection,
    owner: &str,
    kind: &str,
) -> CwpdbResult<Vec<OrderedMember>> {
    let mut statement=db.prepare_cached("SELECT t.canonical_ref,r.canonical_ref,json(e.props) FROM edges e JOIN ref_bindings t ON t.ref_id=e.target_id JOIN ref_bindings r ON r.ref_id=e.ref_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?) AND e.kind=? ORDER BY e.position,e.ref_id LIMIT 1001")?;
    let rows = statement
        .query_map(params![owner, kind], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() > 1000 {
        return Err("ordered membership work limit exceeded".into());
    }
    rows.into_iter()
        .map(|(target, edge_ref, json)| {
            Ok(OrderedMember {
                target,
                edge_ref,
                properties: serde_json::from_str(&json)?,
            })
        })
        .collect()
}
/// Central storage primitive inside the CWR-owned revision-check transaction.
/// CWR validates declared endpoints and permissions before calling. Existing
/// canonical edge refs survive order changes; no implicit incident cascade.
pub fn replace_ordered_members(
    tx: &Transaction<'_>,
    owner: &str,
    kind: &str,
    schema: &str,
    members: &[OrderedMember],
) -> CwpdbResult<()> {
    use std::collections::BTreeSet;
    if members.len() > 1000 {
        return Err("ordered membership input limit exceeded".into());
    }
    let mut targets = BTreeSet::new();
    let mut edges = BTreeSet::new();
    let mut bytes = 0usize;
    for member in members {
        if !targets.insert(&member.target)
            || !edges.insert(&member.edge_ref)
            || !member.properties.is_object()
        {
            return Err("invalid or duplicate ordered member".into());
        }
        bytes += serde_json::to_vec(&member.properties)?.len();
        if bytes > 1_048_576 {
            return Err("ordered membership byte limit exceeded".into());
        }
    }
    let existing = read_ordered_members(tx, owner, kind)?;
    for member in &existing {
        let incident: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?) OR target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?))",params![member.edge_ref,member.edge_ref],|r|r.get(0))?;
        if incident {
            return Err("ordered edge has incident relationships".into());
        }
    }
    let owner_id = crate::bind(tx, owner)?;
    let schema_id = crate::bind(tx, schema)?;
    for old in &existing {
        if !targets.contains(&old.target) {
            tx.execute("DELETE FROM edges WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?)",[&old.edge_ref])?;
        }
    }
    for (position, member) in members.iter().enumerate() {
        let target = crate::bind(tx, &member.target)?;
        let edge = crate::bind(tx, &member.edge_ref)?;
        let changed=tx.execute("INSERT INTO edges(ref_id,kind,schema_id,source_id,target_id,position,props) VALUES(?,?,?,?,?,?,jsonb(?)) ON CONFLICT(ref_id) DO UPDATE SET position=excluded.position,props=excluded.props,revision=edges.revision+1 WHERE edges.kind=excluded.kind AND edges.schema_id=excluded.schema_id AND edges.source_id=excluded.source_id AND edges.target_id=excluded.target_id",params![edge,kind,schema_id,owner_id,target,position as i64,serde_json::to_string(&member.properties)?])?;
        if changed != 1 {
            return Err("ordered edge identity conflict".into());
        }
    }
    Ok(())
}
