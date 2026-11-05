//! Workloop-owned personal collections retained on the Desktop graph.
//! Membership changes display only; conversation ownership and access remain in CWR.
use crate::{
    self as cwpdb,
    ref_read::{RefReadError, RefRecord},
    CwpdbResult, OrderedMember,
};
use crate::{
    declared::{Catalog, EntityBinding as EntityCatalog},
    sidebar_pins::{self, PinKey},
};
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const SCOPE: &str = "desktop_personal_collection_scope_v1";
const COLLECTION: &str = "desktop_personal_collection_v1";
const LIST_EDGE: &str = "desktop_personal_collection_list_v1";
const ITEM_EDGE: &str = "desktop_personal_collection_item_v1";
const MAX_COLLECTIONS: usize = 32;
const MAX_MEMBERS: usize = 200;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub workspace: String,
    pub viewer: String,
    /// Empty only for a Workspace without an installed owner context.
    pub owner_context_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub r#ref: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSnapshot {
    pub revision: String,
    pub collections: Vec<Collection>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MembersSnapshot {
    pub revision: String,
    pub members: Vec<PinKey>,
}

fn schema(kind: &str) -> String {
    format!("cwp://org.coworkerprotocol/cwpdb/schemas/{kind}/2026-09")
}
fn graph_ref(kind: &str, keys: Value) -> String {
    format!(
        "cwp://app.getworkloop.local/{kind}/{}",
        hex::encode(Sha256::digest(keys.to_string().as_bytes()))
    )
}
fn catalog(declarations: &Catalog, kind: &str) -> CwpdbResult<EntityCatalog> {
    if !matches!(kind, SCOPE | COLLECTION) {
        return Err("unsupported personal collection declaration".into());
    }
    declarations.entity(&schema(kind))
}
fn scope_ref(declarations: &Catalog, scope: &Scope) -> CwpdbResult<String> {
    if scope.workspace.is_empty() || scope.workspace.len() > 16384 {
        return Err("invalid collection workspace".into());
    }
    declarations.validate_reference(&scope.viewer)?;
    if !scope.owner_context_ref.is_empty() {
        if let Some(id) = scope.owner_context_ref.strip_prefix("urn:uuid:") {
            let parsed =
                uuid::Uuid::parse_str(id).map_err(|_| "invalid collection owner context UUID")?;
            if parsed.hyphenated().to_string() != id.to_ascii_lowercase() {
                return Err("invalid collection owner context UUID".into());
            }
        } else {
            declarations.validate_reference(&scope.owner_context_ref)?;
        }
    }
    Ok(graph_ref(
        "personal-collection-scope",
        json!([scope.workspace, scope.viewer, scope.owner_context_ref]),
    ))
}
fn clean_name(name: &str) -> CwpdbResult<String> {
    let clean = name.trim();
    if clean.is_empty()
        || clean.chars().count() > 80
        || clean.chars().any(char::is_control)
        || ["pinned", "recent", "all", "all conversations"].contains(&clean.to_lowercase().as_str())
    {
        return Err("invalid collection name".into());
    }
    Ok(clean.to_owned())
}
fn read_scope(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    scope: &Scope,
    reference: &str,
) -> CwpdbResult<Option<RefRecord>> {
    match cwpdb::ref_read::get_ref(tx, reference) {
        Ok(_) => {
            let record =
                catalog(declarations, SCOPE)?.get_in_connection(tx, &schema(SCOPE), reference)?;
            let value = &record.value.properties["record"];
            if value["workspace"] != scope.workspace
                || value["viewer"] != scope.viewer
                || value["ownerContextRef"] != scope.owner_context_ref
            {
                return Err("retained collection scope mismatch".into());
            }
            Ok(Some(record))
        }
        Err(RefReadError::NotFound) => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn revision(record: Option<&RefRecord>) -> CwpdbResult<String> {
    let value = record
        .and_then(|r| r.value.properties["record"]["revision"].as_str())
        .unwrap_or("0");
    let counter = value.parse::<u64>()?;
    if counter.to_string() != value {
        return Err("invalid collection revision".into());
    }
    Ok(value.to_owned())
}
fn listed(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    scope: &Scope,
) -> CwpdbResult<(String, Vec<Collection>)> {
    let reference = scope_ref(declarations, scope)?;
    let scope_record = read_scope(declarations, tx, scope, &reference)?;
    let edges = cwpdb::read_ordered_members(tx, &reference, LIST_EDGE)?;
    if edges.len() > MAX_COLLECTIONS || (scope_record.is_none() && !edges.is_empty()) {
        return Err("invalid retained collection list".into());
    }
    let mut collections = Vec::with_capacity(edges.len());
    for edge in edges {
        let record = catalog(declarations, COLLECTION)?.get_in_connection(
            tx,
            &schema(COLLECTION),
            &edge.target,
        )?;
        let value = &record.value.properties["record"];
        if value["scopeRef"] != reference {
            return Err("retained collection owner mismatch".into());
        }
        let name = value["name"]
            .as_str()
            .ok_or("invalid retained collection name")?;
        collections.push(Collection {
            r#ref: edge.target,
            name: clean_name(name)?,
        });
    }
    Ok((revision(scope_record.as_ref())?, collections))
}
pub fn read_list(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    scope: &Scope,
) -> CwpdbResult<ListSnapshot> {
    let (revision, collections) = listed(declarations, tx, scope)?;
    Ok(ListSnapshot {
        revision,
        collections,
    })
}
pub fn read_members(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    scope: &Scope,
    collection_ref: &str,
) -> CwpdbResult<MembersSnapshot> {
    let (revision, collections) = listed(declarations, tx, scope)?;
    if !collections
        .iter()
        .any(|collection| collection.r#ref == collection_ref)
    {
        return Err("collection is unavailable".into());
    }
    let edges = cwpdb::read_ordered_members(tx, collection_ref, ITEM_EDGE)?;
    if edges.len() > MAX_MEMBERS {
        return Err("invalid retained collection membership".into());
    }
    let members = edges
        .into_iter()
        .map(|edge| {
            let key: PinKey = serde_json::from_value(edge.properties["record"].clone())?;
            if sidebar_pins::target_ref(declarations, &key)? != edge.target {
                return Err("retained collection target mismatch".into());
            }
            Ok(key)
        })
        .collect::<CwpdbResult<Vec<_>>>()?;
    Ok(MembersSnapshot { revision, members })
}
fn replace_list(
    _declarations: &Catalog,
    tx: &Transaction<'_>,
    scope_ref: &str,
    collections: &[Collection],
) -> CwpdbResult<()> {
    let edges = collections
        .iter()
        .map(|item| OrderedMember {
            target: item.r#ref.clone(),
            edge_ref: graph_ref(LIST_EDGE, json!([scope_ref, item.r#ref])),
            properties: json!({"record":{}}),
        })
        .collect::<Vec<_>>();
    cwpdb::replace_ordered_members(tx, scope_ref, LIST_EDGE, &schema(LIST_EDGE), &edges)?;
    Ok(())
}
fn replace_members(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    collection_ref: &str,
    members: &[PinKey],
) -> CwpdbResult<()> {
    let edges = members
        .iter()
        .map(|key| {
            let target = sidebar_pins::target_ref(declarations, key)?;
            Ok(OrderedMember {
                edge_ref: graph_ref(ITEM_EDGE, json!([collection_ref, target])),
                target,
                properties: json!({"record":key}),
            })
        })
        .collect::<CwpdbResult<Vec<_>>>()?;
    cwpdb::replace_ordered_members(tx, collection_ref, ITEM_EDGE, &schema(ITEM_EDGE), &edges)?;
    Ok(())
}
fn bump_scope(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    scope: &Scope,
    scope_ref: &str,
) -> CwpdbResult<()> {
    let before = read_scope(declarations, tx, scope, scope_ref)?;
    let old_revision = revision(before.as_ref())?;
    let next = old_revision
        .parse::<u64>()?
        .checked_add(1)
        .ok_or("collection revision exhausted")?;
    let value = json!({"record":{
        "workspace":scope.workspace,
        "viewer":scope.viewer,
        "ownerContextRef":scope.owner_context_ref,
        "revision":next.to_string()
    }});
    let catalog = catalog(declarations, SCOPE)?;
    if let Some(before) = before {
        catalog.replace_in_transaction(tx, &schema(SCOPE), scope_ref, &before.revision, &value)?;
    } else {
        catalog.create_in_transaction(tx, &schema(SCOPE), scope_ref, &value)?;
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Change {
    Create {
        name: String,
    },
    Rename {
        collection_ref: String,
        name: String,
    },
    Delete {
        collection_ref: String,
    },
    MoveCollection {
        collection_ref: String,
        target_ref: String,
        before: bool,
    },
    MoveMember {
        collection_ref: String,
        key: PinKey,
        target: PinKey,
        before: bool,
    },
    SetMember {
        collection_ref: String,
        key: PinKey,
        present: bool,
    },
}
pub fn apply_change(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    scope: &Scope,
    change: &Change,
    request_id: &str,
) -> CwpdbResult<String> {
    let scope_ref = scope_ref(declarations, scope)?;
    let (_, mut collections) = listed(declarations, tx, scope)?;
    let (collection_ref, changed) = match change {
        Change::Create { name } => {
            let name = clean_name(name)?;
            if collections.len() >= MAX_COLLECTIONS {
                return Err("collection limit exceeded".into());
            }
            if collections
                .iter()
                .any(|c| c.name.to_lowercase() == name.to_lowercase())
            {
                return Err("collection name already exists".into());
            }
            let reference = graph_ref(
                "personal-conversation-collection",
                json!([scope_ref, request_id]),
            );
            catalog(declarations, COLLECTION)?.create_in_transaction(
                tx,
                &schema(COLLECTION),
                &reference,
                &json!({"record":{"scopeRef":scope_ref,"name":name}}),
            )?;
            collections.push(Collection {
                r#ref: reference.clone(),
                name,
            });
            // The scope node must precede graph edges.
            bump_scope(declarations, tx, scope, &scope_ref)?;
            replace_list(declarations, tx, &scope_ref, &collections)?;
            return Ok(reference);
        }
        Change::Rename {
            collection_ref,
            name,
        } => {
            let name = clean_name(name)?;
            let current = collections
                .iter()
                .find(|c| c.r#ref == *collection_ref)
                .ok_or("collection is unavailable")?;
            if collections
                .iter()
                .any(|c| c.r#ref != *collection_ref && c.name.to_lowercase() == name.to_lowercase())
            {
                return Err("collection name already exists".into());
            }
            if current.name == name {
                (collection_ref.clone(), false)
            } else {
                let before = catalog(declarations, COLLECTION)?.get_in_connection(
                    tx,
                    &schema(COLLECTION),
                    collection_ref,
                )?;
                catalog(declarations, COLLECTION)?.replace_in_transaction(
                    tx,
                    &schema(COLLECTION),
                    collection_ref,
                    &before.revision,
                    &json!({"record":{"scopeRef":scope_ref,"name":name}}),
                )?;
                (collection_ref.clone(), true)
            }
        }
        Change::Delete { collection_ref } => {
            if !collections.iter().any(|c| c.r#ref == *collection_ref) {
                return Err("collection is unavailable".into());
            }
            replace_members(declarations, tx, collection_ref, &[])?;
            collections.retain(|c| c.r#ref != *collection_ref);
            replace_list(declarations, tx, &scope_ref, &collections)?;
            let before = catalog(declarations, COLLECTION)?.get_in_connection(
                tx,
                &schema(COLLECTION),
                collection_ref,
            )?;
            catalog(declarations, COLLECTION)?.remove_in_transaction(
                tx,
                &schema(COLLECTION),
                collection_ref,
                &before.revision,
            )?;
            (collection_ref.clone(), true)
        }
        Change::MoveCollection {
            collection_ref,
            target_ref,
            before,
        } => {
            let changed = ordering::move_collection(
                declarations,
                tx,
                &scope_ref,
                &mut collections,
                collection_ref,
                target_ref,
                *before,
            )?;
            (collection_ref.clone(), changed)
        }
        Change::MoveMember {
            collection_ref,
            key,
            target,
            before,
        } => {
            let changed = ordering::move_member(
                declarations,
                tx,
                scope,
                collection_ref,
                key,
                target,
                *before,
            )?;
            (collection_ref.clone(), changed)
        }
        Change::SetMember {
            collection_ref,
            key,
            present,
        } => {
            if !collections.iter().any(|c| c.r#ref == *collection_ref) {
                return Err("collection is unavailable".into());
            }
            let mut members = read_members(declarations, tx, scope, collection_ref)?.members;
            let target = sidebar_pins::target_ref(declarations, key)?;
            let mut moved = false;
            if *present {
                for collection in &collections {
                    if collection.r#ref == *collection_ref {
                        continue;
                    }
                    let mut others =
                        read_members(declarations, tx, scope, &collection.r#ref)?.members;
                    let previous_len = others.len();
                    others.retain(|member| {
                        sidebar_pins::target_ref(declarations, member).ok().as_ref()
                            != Some(&target)
                    });
                    if others.len() != previous_len {
                        replace_members(declarations, tx, &collection.r#ref, &others)?;
                        moved = true;
                    }
                }
            }
            let existing = members.iter().position(|member| {
                sidebar_pins::target_ref(declarations, member).ok().as_ref() == Some(&target)
            });
            if *present && existing.is_none() {
                if members.len() >= MAX_MEMBERS {
                    return Err("collection member limit exceeded".into());
                }
                sidebar_pins::ensure_target(declarations, tx, key)?;
                members.push(key.clone());
                replace_members(declarations, tx, collection_ref, &members)?;
                (collection_ref.clone(), true)
            } else if !*present && existing.is_some() {
                members.remove(existing.unwrap());
                replace_members(declarations, tx, collection_ref, &members)?;
                (collection_ref.clone(), true)
            } else {
                (collection_ref.clone(), moved)
            }
        }
    };
    if changed {
        bump_scope(declarations, tx, scope, &scope_ref)?;
    }
    Ok(collection_ref)
}

pub fn mutate_once(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    scope: &Scope,
    change: Change,
    request_id: &str,
) -> CwpdbResult<Option<String>> {
    use cwpdb::sequence_receipt::{self, Replay};
    let reference = scope_ref(declarations, scope)?;
    let args = json!({"scope":scope,"change":change});
    match sequence_receipt::check(tx, &reference, request_id, &args)? {
        Replay::Retained(value) => return Ok(Some(serde_json::from_value(value)?)),
        Replay::CommittedResultExpired => return Ok(None),
        Replay::NotCommitted => {}
    }
    let result = apply_change(declarations, tx, scope, &change, request_id)?;
    sequence_receipt::commit(tx, &reference, request_id, &args, &json!(result), &[])?;
    Ok(Some(result))
}

#[cfg(test)]
#[path = "personal_collections_tests.rs"]
mod tests;

#[path = "personal_collection_ordering.rs"]
mod ordering;
