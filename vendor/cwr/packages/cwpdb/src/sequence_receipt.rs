//! Graph-backed local mutation replay. The marker is content-free and durable;
//! payload sits on a disposable edge chain dependent on every retained event.
//! Purging any dependency removes the terminal payload through shared lifecycle.
use crate::{identity::claim_record_identity, BindingRetention, CwpdbResult};
use rusqlite::{params, Connection, Transaction};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[derive(Debug)]
pub struct RequestConflict;
impl std::fmt::Display for RequestConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("mutation request conflict")
    }
}
impl std::error::Error for RequestConflict {}
const KIND: &str = "sequence_mutation_receipt_v1";
fn key(scope: &str, request: &str) -> String {
    hex::encode(Sha256::digest(
        json!([scope, request]).to_string().as_bytes(),
    ))
}
pub(crate) fn marker(scope: &str, request: &str) -> String {
    format!(
        "cwp://org.coworkerprotocol/mutation-markers/{}",
        key(scope, request)
    )
}
pub(crate) fn tip(scope: &str, request: &str) -> String {
    format!(
        "cwp://org.coworkerprotocol/mutation-results/{}",
        key(scope, request)
    )
}
pub fn argument_digest(value: &Value) -> String {
    hex::encode(Sha256::digest(value.to_string().as_bytes()))
}
#[derive(Debug, PartialEq)]
pub enum Replay {
    NotCommitted,
    Retained(Value),
    CommittedResultExpired,
}
pub fn check(db: &Connection, scope: &str, request: &str, args: &Value) -> CwpdbResult<Replay> {
    if request.is_empty() || request.len() > 128 {
        return Err("invalid mutation request ID".into());
    }
    if crate::node_props(db, &marker(scope, request))?.is_none() {
        return Ok(Replay::NotCommitted);
    }
    let result:Option<String>=db.query_row("SELECT json(props) FROM edges WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?)",[tip(scope,request)],|r|r.get(0)).optional()?;
    let Some(result) = result else {
        return Ok(Replay::CommittedResultExpired);
    };
    let result: Value = serde_json::from_str(&result)?;
    if result["arguments"] != argument_digest(args) {
        return Err(RequestConflict.into());
    }
    Ok(Replay::Retained(result["result"].clone()))
}
use rusqlite::OptionalExtension;
/// Commit only after the owning mutation, in the same immediate transaction.
/// Result is <=512 KiB; no HTTP or caller transaction crosses this boundary.
pub fn commit(
    tx: &Transaction<'_>,
    scope: &str,
    request: &str,
    args: &Value,
    result: &Value,
    dependencies: &[String],
) -> CwpdbResult<()> {
    if request.is_empty()
        || request.len() > 128
        || dependencies.len() > 100
        || serde_json::to_vec(result)?.len() > 524288
    {
        return Err("mutation receipt limit exceeded".into());
    }
    let marker = marker(scope, request);
    if crate::node_props(tx, &marker)?.is_some() {
        return Err("mutation already committed".into());
    }
    crate::put_node(tx, &marker, "mutation_marker_v1", &json!({}))?;
    let mut previous = marker;
    if dependencies.is_empty() {
        crate::bind(tx, scope)?;
    }
    let fallback = [scope.to_string()];
    let selected = if dependencies.is_empty() {
        &fallback[..]
    } else {
        dependencies
    };
    for (index, target) in selected.iter().enumerate() {
        let last = index + 1 == selected.len();
        let reference = if last {
            tip(scope, request)
        } else {
            crate::derived_ref(KIND, &[&previous, target])
        };
        let edge = claim_record_identity(
            tx,
            &reference,
            &format!("edge:{KIND}"),
            BindingRetention::Disposable,
        )?;
        // The marker is already durable; event/edge endpoints are already bound.
        // Never create an unknown dependency as an accidentally durable binding.
        let source = crate::find(tx, &previous)?.ok_or("missing receipt source")?;
        let target_id = crate::find(tx, target)?.ok_or("missing receipt dependency")?;
        let schema = crate::bind(tx, &crate::schema_ref(KIND))?;
        let props = if last {
            json!({"arguments":argument_digest(args),"result":result})
        } else {
            json!({})
        };
        tx.execute("INSERT INTO edges(ref_id,kind,schema_id,source_id,target_id,props) VALUES(?,?,?,?,?,jsonb(?))",params![edge,KIND,schema,source,target_id,props.to_string()])?;
        previous = reference;
    }
    // Reject receipt fan-out before accepting a request that would make one
    // event impossible to erase under the shared 1024-incident-work contract.
    for dependency in dependencies
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
    {
        let work:i64=tx.query_row("WITH RECURSIVE doomed(id) AS (SELECT ref_id FROM ref_bindings WHERE canonical_ref=? UNION SELECT e.ref_id FROM edges e JOIN doomed d ON e.source_id=d.id OR e.target_id=d.id LIMIT 1025) SELECT count(*) FROM doomed",[dependency],|r|r.get(0))?;
        if work > 1024 {
            return Err("receipt dependency work limit exceeded".into());
        }
    }
    Ok(())
}

/// Internal reconciliation read. The declared operation must recheck current
/// authorization before using this; it never grants replay or mutation rights.
pub(crate) fn retained_result(db: &Connection, scope: &str, request: &str) -> CwpdbResult<Replay> {
    if request.is_empty() || request.len() > 128 {
        return Err("invalid mutation request ID".into());
    }
    if crate::node_props(db, &marker(scope, request))?.is_none() {
        return Ok(Replay::NotCommitted);
    }
    let payload: Option<String> = db.query_row(
        "SELECT json(props) FROM edges WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?)",
        [tip(scope, request)], |row|row.get(0),
    ).optional()?;
    let Some(payload) = payload else {
        return Ok(Replay::CommittedResultExpired);
    };
    let payload: Value = serde_json::from_str(&payload)?;
    Ok(Replay::Retained(payload["result"].clone()))
}
