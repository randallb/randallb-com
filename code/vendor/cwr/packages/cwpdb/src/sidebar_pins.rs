//! Declared local sidebar pins. Existing domain owns viewer/visibility authority.
//! This internal transaction module does not claim retained request-ID replay.
use crate::declared::{Catalog, EntityBinding as EntityCatalog};
use crate::{
    self as cwpdb,
    ref_read::{RefReadError, RefRecord},
    CwpdbResult, OrderedMember,
};
use rusqlite::{Connection, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
#[derive(Debug)]
pub struct RevisionConflict;
impl std::fmt::Display for RevisionConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("pin revision conflict")
    }
}
impl std::error::Error for RevisionConflict {}
const KIND: &str = "desktop_sidebar_pin_v1";
const SCOPE: &str = "desktop_sidebar_pin_scope_v1";
const TARGET: &str = "desktop_source_conversation_v1";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PinKey {
    pub source_ref: String,
    pub r#ref: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinScope {
    pub workspace: String,
    pub viewer: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct PinSnapshot {
    pub revision: String,
    pub members: Vec<PinKey>,
}
fn graph_ref(kind: &str, keys: Value) -> String {
    format!(
        "cwp://app.getworkloop.local/{kind}/{}",
        hex::encode(Sha256::digest(keys.to_string().as_bytes()))
    )
}
fn schema(kind: &str) -> String {
    format!("cwp://org.coworkerprotocol/cwpdb/schemas/{kind}/2026-09")
}
fn catalog(declarations: &Catalog, kind: &str) -> CwpdbResult<EntityCatalog> {
    if !matches!(kind, SCOPE | TARGET) {
        return Err("unsupported pin declaration".into());
    }
    declarations.entity(&schema(kind))
}
pub(crate) fn scope_ref(declarations: &Catalog, scope: &PinScope) -> CwpdbResult<String> {
    if scope.workspace.is_empty() || scope.workspace.len() > 16384 {
        return Err("invalid pin workspace".into());
    }
    declarations.validate_reference(&scope.viewer)?;
    Ok(graph_ref(
        "sidebar-pin-scope",
        json!([scope.workspace, scope.viewer]),
    ))
}
pub(crate) fn target_ref(declarations: &Catalog, key: &PinKey) -> CwpdbResult<String> {
    for reference in [&key.source_ref, &key.r#ref] {
        declarations.validate_reference(reference)?;
        if reference.contains("/projection/") || reference.len() > 16384 {
            return Err("invalid pin target".into());
        }
    }
    Ok(graph_ref(
        "source-conversation",
        json!([key.source_ref, key.r#ref]),
    ))
}
pub(crate) fn ensure_target(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    key: &PinKey,
) -> CwpdbResult<String> {
    let target = target_ref(declarations, key)?;
    let target_catalog = catalog(declarations, TARGET)?;
    match cwpdb::ref_read::get_ref(tx, &target) {
        Ok(_) => {
            let retained = target_catalog.get_in_connection(tx, &schema(TARGET), &target)?;
            if retained.value.properties != json!({"record":key}) {
                return Err("pin target value conflict".into());
            }
        }
        Err(RefReadError::NotFound) => {
            target_catalog.create_in_transaction(
                tx,
                &schema(TARGET),
                &target,
                &json!({"record":key}),
            )?;
        }
        Err(error) => return Err(error.into()),
    }
    let kind = "desktop_conversation_resource_v1";
    cwpdb::replace_ordered_members(
        tx,
        &target,
        kind,
        &schema(kind),
        &[OrderedMember {
            target: key.r#ref.clone(),
            edge_ref: graph_ref(kind, json!([target, key.r#ref])),
            properties: json!({"record":{}}),
        }],
    )?;
    Ok(target)
}
fn current(
    declarations: &Catalog,
    db: &Connection,
    reference: &str,
) -> CwpdbResult<Option<RefRecord>> {
    match cwpdb::ref_read::get_ref(db, reference) {
        Ok(_) => Ok(Some(catalog(declarations, SCOPE)?.get_in_connection(
            db,
            &schema(SCOPE),
            reference,
        )?)),
        Err(RefReadError::NotFound) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub fn read(
    declarations: &Catalog,
    db: &Transaction<'_>,
    scope: &PinScope,
) -> CwpdbResult<PinSnapshot> {
    let reference = scope_ref(declarations, scope)?;
    let record = current(declarations, db, &reference)?;
    if let Some(record) = &record {
        let r = &record.value.properties["record"];
        if r["workspace"] != scope.workspace || r["viewer"] != scope.viewer {
            return Err("retained pin scope mismatch".into());
        }
    }
    let revision = match record {
        Some(record) => record.value.properties["record"]["revision"]
            .as_str()
            .ok_or("invalid retained pin revision")?
            .to_owned(),
        None => "0".into(),
    };
    let counter = revision.parse::<u64>()?;
    if counter.to_string() != revision {
        return Err("invalid retained pin revision".into());
    }
    let members = cwpdb::read_ordered_members(db, &reference, KIND)?
        .into_iter()
        .map(|edge| {
            let key: PinKey = serde_json::from_value(edge.properties["record"].clone())?;
            if target_ref(declarations, &key)? != edge.target {
                return Err("retained pin target mismatch".into());
            }
            Ok(key)
        })
        .collect::<CwpdbResult<Vec<_>>>()?;
    Ok(PinSnapshot { revision, members })
}
/// Must be called inside the facade's one immediate transaction. All validation
/// precedes mutation. Failure propagates so the owning transaction rolls back.
pub fn replace_in_transaction(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    scope: &PinScope,
    expected: &str,
    members: &[PinKey],
) -> CwpdbResult<PinSnapshot> {
    if members.len() > 1000 {
        return Err("pin input limit exceeded".into());
    }
    let reference = scope_ref(declarations, scope)?;
    let mut unique = BTreeSet::new();
    let mut targets = Vec::new();
    let mut bytes = 0usize;
    for key in members {
        let target = target_ref(declarations, key)?;
        if !unique.insert(target.clone()) {
            return Err("duplicate pin target".into());
        }
        bytes += serde_json::to_vec(key)?.len();
        if bytes > 524288 {
            return Err("pin byte limit exceeded".into());
        }
        targets.push(target);
    }
    let before = read(declarations, tx, scope)?;
    if before.revision != expected {
        return Err(RevisionConflict.into());
    }
    if before.members == members {
        return Ok(before);
    }
    let revision = expected
        .parse::<u64>()?
        .checked_add(1)
        .ok_or("pin revision exhausted")?
        .to_string();
    let old = current(declarations, tx, &reference)?;
    let scope_catalog = catalog(declarations, SCOPE)?;
    let value =
        json!({"record":{"workspace":scope.workspace,"viewer":scope.viewer,"revision":revision}});
    match old {
        Some(old) => {
            scope_catalog.replace_in_transaction(
                tx,
                &schema(SCOPE),
                &reference,
                &old.revision,
                &value,
            )?;
        }
        None => {
            scope_catalog.create_in_transaction(tx, &schema(SCOPE), &reference, &value)?;
        }
    }
    for (key, target) in members.iter().zip(&targets) {
        if &ensure_target(declarations, tx, key)? != target {
            return Err("pin target identity changed".into());
        }
    }
    let edges = members
        .iter()
        .zip(targets)
        .map(|(key, target)| OrderedMember {
            edge_ref: graph_ref(KIND, json!([reference, target])),
            target,
            properties: json!({"record":key}),
        })
        .collect::<Vec<_>>();
    cwpdb::replace_ordered_members(tx, &reference, KIND, &schema(KIND), &edges)?;
    read(declarations, tx, scope)
}
#[cfg(test)]
#[path = "sidebar_pins_tests.rs"]
mod tests;

#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PinReplacement {
    Committed { snapshot: PinSnapshot },
    CommittedResultExpired,
}
/// Exact local request replay after current owner authorization. The scope's
/// identity binds request IDs to workspace/viewer, not to a global caller pool.
pub fn replace_once_in_transaction(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    scope: &PinScope,
    expected: &str,
    members: &[PinKey],
    request_id: &str,
) -> CwpdbResult<PinReplacement> {
    use cwpdb::sequence_receipt::{self, Replay};
    let reference = scope_ref(declarations, scope)?;
    if members.len() > 1000 || serde_json::to_vec(members)?.len() > 524288 {
        return Err("pin input limit exceeded".into());
    }
    let args = json!({"scope":scope,"expected":expected,"members":members});
    match sequence_receipt::check(tx, &reference, request_id, &args)? {
        Replay::Retained(value) => {
            return Ok(PinReplacement::Committed {
                snapshot: serde_json::from_value(value)?,
            })
        }
        Replay::CommittedResultExpired => return Ok(PinReplacement::CommittedResultExpired),
        Replay::NotCommitted => {}
    }
    let snapshot = replace_in_transaction(declarations, tx, scope, expected, members)?;
    sequence_receipt::commit(
        tx,
        &reference,
        request_id,
        &args,
        &serde_json::to_value(&snapshot)?,
        &[],
    )?;
    Ok(PinReplacement::Committed { snapshot })
}
