//! Owner-only atomic Desktop registry. Paths are supplied after Desktop's
//! canonicalization/copy-vs-move decision. No filesystem operation runs in a tx.
use super::{
    receipts::{Receipts, Replay},
    OperationError, OperationErrorCode as Code,
};
use crate::{
    self as cwpdb,
    declared::{Catalog as Declarations, EntityBinding as EntityCatalog},
    ref_read::{RefReadError, RefRecord},
    unordered::{self, Cardinality, Contract},
    NativeStore,
};
use rusqlite::{Connection, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::json;

const WORKSPACE: &str = "cwp://org.coworkerprotocol/cwr/entities/desktop-workspace";
const BINDING: &str = "cwp://org.coworkerprotocol/cwr/entities/desktop-workspace-binding";
const WORKSPACE_KIND: &str = "desktop_workspace_v1";
const BINDING_KIND: &str = "desktop_workspace_binding_v1";
const BINDING_EDGE: Contract = Contract {
    kind: BINDING_KIND,
    schema: "cwp://org.coworkerprotocol/cwpdb/schemas/desktop_workspace_binding_v1/2026-09",
    source_kind: WORKSPACE_KIND,
    target_kind: BINDING_KIND,
    external_target: false,
    source_cardinality: Cardinality::One,
    target_cardinality: Cardinality::One,
};
const INSTANCE_EDGE: Contract = Contract {
    kind: "desktop_workspace_instance_v1",
    schema: "cwp://org.coworkerprotocol/cwpdb/schemas/desktop_workspace_instance_v1/2026-09",
    source_kind: WORKSPACE_KIND,
    target_kind: "desktop_workspace_instance_v1",
    external_target: true,
    source_cardinality: Cardinality::One,
    target_cardinality: Cardinality::One,
};

type Result<T> = std::result::Result<T, OperationError>;
fn error(code: Code) -> OperationError {
    OperationError { code }
}
fn storage(_: impl std::fmt::Display) -> OperationError {
    error(Code::StorageFailure)
}
fn relationship(e: unordered::Error) -> OperationError {
    error(match e {
        unordered::Error::RevisionConflict => Code::RevisionConflict,
        unordered::Error::RelationshipsExist => Code::RelationshipsExist,
        unordered::Error::Storage(_) => Code::StorageFailure,
        _ => Code::UnsupportedDomainContract,
    })
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    ReadRoot {
        root: String,
    },
    ReadId {
        id: String,
    },
    Ensure {
        root: String,
        id: String,
        request_id: String,
    },
    Register {
        root: String,
        id: String,
        request_id: String,
    },
    Rebind {
        root: String,
        id: String,
        expected_revision: String,
        request_id: String,
    },
    Remove {
        root: String,
        id: String,
        expected_revision: String,
        request_id: String,
    },
}
impl Request {
    pub fn is_write(&self) -> bool {
        !matches!(self, Self::ReadRoot { .. } | Self::ReadId { .. })
    }
    fn request_id(&self) -> Option<&str> {
        match self {
            Self::Ensure { request_id, .. }
            | Self::Register { request_id, .. }
            | Self::Rebind { request_id, .. }
            | Self::Remove { request_id, .. } => Some(request_id),
            _ => None,
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub root: String,
    pub revision: String,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Read { entry: Option<Entry> },
    Committed { entry: Option<Entry>, removed: bool },
    CommittedResultExpired,
}

struct Catalog {
    workspace: EntityCatalog,
    binding: EntityCatalog,
    receipts: Receipts,
}
impl Catalog {
    fn new(declarations: &Declarations) -> Result<Self> {
        Ok(Self {
            workspace: declarations
                .entity("cwp://org.coworkerprotocol/cwr/entities/desktop-workspace")
                .map_err(storage)?,
            binding: declarations
                .entity("cwp://org.coworkerprotocol/cwr/entities/desktop-workspace-binding")
                .map_err(storage)?,
            receipts: Receipts::new(declarations, WORKSPACE_KIND)?,
        })
    }
    fn read(
        &self,
        db: &Connection,
        definition: &str,
        reference: &str,
    ) -> Result<Option<RefRecord>> {
        match cwpdb::ref_read::get_ref(db, reference) {
            Err(RefReadError::NotFound) => Ok(None),
            Err(RefReadError::WrongRecordType) => Err(error(Code::UnsupportedDomainContract)),
            Err(e) => Err(storage(e)),
            Ok(_) => {
                let catalog = match definition {
                    WORKSPACE => &self.workspace,
                    BINDING => &self.binding,
                    _ => return Err(error(Code::UnsupportedDomainContract)),
                };
                catalog
                    .get_in_connection(db, definition, reference)
                    .map(Some)
                    .map_err(|_| error(Code::UnsupportedDomainContract))
            }
        }
    }
    fn entry(&self, db: &Connection, id: &str) -> Result<Option<Entry>> {
        validate_id(id)?;
        let Some(record) = self.read(db, WORKSPACE, id)? else {
            return Ok(None);
        };
        let value = &record.value.properties["record"];
        if value["id"] != id {
            return Err(error(Code::UnsupportedDomainContract));
        }
        let root = value["root"]
            .as_str()
            .ok_or_else(|| error(Code::UnsupportedDomainContract))?;
        validate_root(root)?;
        let binding = self
            .read(db, BINDING, &binding_ref(root))?
            .ok_or_else(|| error(Code::UnsupportedDomainContract))?;
        if binding.value.properties["record"] != *value {
            return Err(error(Code::UnsupportedDomainContract));
        }
        for (contract, target) in [
            (BINDING_EDGE, binding_ref(root)),
            (INSTANCE_EDGE, instance_ref(root)),
        ] {
            let edge = unordered::single_outgoing(db, contract, id)
                .map_err(relationship)?
                .ok_or_else(|| error(Code::UnsupportedDomainContract))?;
            if edge.target != target || edge.revision > record.revision.legacy_counter() {
                return Err(error(Code::UnsupportedDomainContract));
            }
        }
        Ok(Some(Entry {
            id: id.into(),
            root: root.into(),
            revision: record.revision.legacy_counter().to_string(),
        }))
    }
    fn root(&self, db: &Connection, root: &str) -> Result<Option<Entry>> {
        validate_root(root)?;
        let Some(record) = self.read(db, BINDING, &binding_ref(root))? else {
            return Ok(None);
        };
        let value = &record.value.properties["record"];
        let Some(id) = value["id"].as_str() else {
            return Ok(None);
        };
        if value["root"] != root {
            return Err(error(Code::UnsupportedDomainContract));
        }
        let entry = self
            .entry(db, id)?
            .ok_or_else(|| error(Code::UnsupportedDomainContract))?;
        if entry.root != root {
            return Err(error(Code::UnsupportedDomainContract));
        }
        Ok(Some(entry))
    }
    fn slot(&self, tx: &Transaction<'_>, root: &str, id: Option<&str>) -> Result<()> {
        let reference = binding_ref(root);
        let value = match id {
            Some(id) => json!({"record":{"id":id,"root":root}}),
            None => json!({"record":{"id":null}}),
        };
        if let Some(before) = self.read(tx, BINDING, &reference)? {
            unordered::adopt_entity(tx, &reference, BINDING_KIND).map_err(storage)?;
            if before.value.properties != value {
                self.binding
                    .replace_in_transaction(tx, BINDING, &reference, &before.revision, &value)
                    .map_err(storage)?;
            }
        } else {
            if unordered::retired(tx, &reference).map_err(storage)? {
                return Err(error(Code::RefRetired));
            }
            self.binding
                .create_in_transaction(tx, BINDING, &reference, &value)
                .map_err(storage)?;
        }
        Ok(())
    }
    fn detach(&self, tx: &Transaction<'_>, contract: Contract, id: &str) -> Result<()> {
        if let Some(edge) = unordered::single_outgoing(tx, contract, id).map_err(relationship)? {
            unordered::unlink(tx, contract, id, &edge.target, edge.revision)
                .map_err(relationship)?;
        }
        Ok(())
    }
    fn mutate(&self, tx: &Transaction<'_>, request: &Request) -> Result<Response> {
        let request_id = request
            .request_id()
            .ok_or_else(|| error(Code::InvalidInput))?;
        let arguments = json!({"domain":"registry","request":request});
        match self.receipts.replay(tx, request_id, &arguments)? {
            Replay::Retained(response) => return Ok(response),
            Replay::Expired => return Ok(Response::CommittedResultExpired),
            Replay::Absent => {}
        }
        let response = match request {
            Request::Ensure { root, id, .. } | Request::Register { root, id, .. } => {
                validate_root(root)?;
                validate_id(id)?;
                if let Some(entry) = self.root(tx, root)? {
                    if matches!(request, Request::Register { .. }) && entry.id != *id {
                        return Err(error(Code::AlreadyExists));
                    }
                    unordered::adopt_entity(tx, &entry.id, WORKSPACE_KIND).map_err(storage)?;
                    self.receipts.expire(tx, &entry.id)?;
                    Response::Committed {
                        entry: self.entry(tx, &entry.id)?,
                        removed: false,
                    }
                } else {
                    if self.read(tx, WORKSPACE, id)?.is_some() {
                        return Err(error(Code::EdgeConflict));
                    }
                    if unordered::retired(tx, id).map_err(storage)? {
                        return Err(error(Code::RefRetired));
                    }
                    self.workspace
                        .create_in_transaction(
                            tx,
                            WORKSPACE,
                            id,
                            &json!({"record":{"id":id,"root":root}}),
                        )
                        .map_err(storage)?;
                    self.slot(tx, root, Some(id))?;
                    unordered::link(tx, BINDING_EDGE, id, &binding_ref(root))
                        .map_err(relationship)?;
                    unordered::link(tx, INSTANCE_EDGE, id, &instance_ref(root))
                        .map_err(relationship)?;
                    Response::Committed {
                        entry: self.entry(tx, id)?,
                        removed: false,
                    }
                }
            }
            Request::Rebind {
                root,
                id,
                expected_revision,
                ..
            } => {
                validate_root(root)?;
                validate_id(id)?;
                let entry = self.entry(tx, id)?.ok_or_else(|| error(Code::NotFound))?;
                if entry.revision != *expected_revision {
                    return Err(error(Code::RevisionConflict));
                }
                if let Some(existing) = self.root(tx, root)? {
                    if existing.id != *id {
                        return Err(error(Code::AlreadyExists));
                    }
                }
                unordered::adopt_entity(tx, id, WORKSPACE_KIND).map_err(storage)?;
                self.receipts.expire(tx, id)?;
                if entry.root != *root {
                    self.detach(tx, BINDING_EDGE, id)?;
                    self.detach(tx, INSTANCE_EDGE, id)?;
                    self.slot(tx, &entry.root, None)?;
                    let before = self
                        .read(tx, WORKSPACE, id)?
                        .ok_or_else(|| error(Code::NotFound))?;
                    self.workspace
                        .replace_in_transaction(
                            tx,
                            WORKSPACE,
                            id,
                            &before.revision,
                            &json!({"record":{"id":id,"root":root}}),
                        )
                        .map_err(storage)?;
                    self.slot(tx, root, Some(id))?;
                    unordered::link(tx, BINDING_EDGE, id, &binding_ref(root))
                        .map_err(relationship)?;
                    unordered::link(tx, INSTANCE_EDGE, id, &instance_ref(root))
                        .map_err(relationship)?;
                }
                Response::Committed {
                    entry: self.entry(tx, id)?,
                    removed: false,
                }
            }
            Request::Remove {
                root,
                id,
                expected_revision,
                ..
            } => {
                validate_root(root)?;
                validate_id(id)?;
                let entry = self.root(tx, root)?.ok_or_else(|| error(Code::NotFound))?;
                if entry.id != *id || entry.revision != *expected_revision {
                    return Err(error(Code::RevisionConflict));
                }
                unordered::adopt_entity(tx, id, WORKSPACE_KIND).map_err(storage)?;
                self.receipts.expire(tx, id)?;
                self.detach(tx, BINDING_EDGE, id)?;
                self.detach(tx, INSTANCE_EDGE, id)?;
                self.slot(tx, root, None)?;
                unordered::delete_unlinked_entity(tx, id).map_err(relationship)?;
                Response::Committed {
                    entry: None,
                    removed: true,
                }
            }
            _ => return Err(error(Code::InvalidInput)),
        };
        let owner = match &response {
            Response::Committed {
                entry: Some(entry), ..
            } => Some(entry.id.as_str()),
            _ => None,
        };
        self.receipts
            .save(tx, request_id, &arguments, &response, owner)?;
        Ok(response)
    }
}

pub fn execute(
    store: &mut NativeStore,
    declarations: &Declarations,
    request: Request,
) -> Result<Response> {
    let catalog = Catalog::new(declarations)?;
    if let Some(id) = request.request_id() {
        uuid::Uuid::parse_str(id).map_err(|_| error(Code::InvalidInput))?;
    }
    let result = if request.is_write() {
        store.graph_transaction(|tx| Ok(catalog.mutate(tx, &request)?))
    } else {
        store.graph_read(|db| {
            Ok(match &request {
                Request::ReadRoot { root } => Response::Read {
                    entry: catalog.root(db, root)?,
                },
                Request::ReadId { id } => Response::Read {
                    entry: catalog.entry(db, id)?,
                },
                _ => return Err(error(Code::InvalidInput).into()),
            })
        })
    };
    result.map_err(|e| match e.downcast::<OperationError>() {
        Ok(e) => *e,
        Err(_) => error(Code::StorageFailure),
    })
}
fn binding_ref(root: &str) -> String {
    unordered::desktop_ref("workspace-binding", &json!(root))
}
fn instance_ref(root: &str) -> String {
    unordered::desktop_ref("workspace-instance", &json!(root))
}
pub(super) fn validate_root(root: &str) -> Result<()> {
    if root.len() > 16384
        || root.contains('\0')
        || !std::path::Path::new(root).is_absolute()
        || root.split('/').any(|part| part == ".." || part == ".")
    {
        return Err(error(Code::InvalidInput));
    }
    Ok(())
}
fn validate_id(id: &str) -> Result<()> {
    let Some((authority, uuid)) = id
        .strip_prefix("cwp://")
        .and_then(|s| s.split_once("/workspaces/"))
    else {
        return Err(error(Code::InvalidInput));
    };
    let host = authority
        .strip_prefix("local.")
        .ok_or_else(|| error(Code::InvalidInput))?;
    if host.is_empty()
        || host.len() > 63
        || !host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || host.starts_with('-')
        || host.ends_with('-')
    {
        return Err(error(Code::InvalidInput));
    }
    let value = uuid::Uuid::parse_str(uuid).map_err(|_| error(Code::InvalidInput))?;
    if !(1..=8).contains(&value.get_version_num())
        || value.get_variant() != uuid::Variant::RFC4122
        || uuid.len() != 36
    {
        return Err(error(Code::InvalidInput));
    }
    Ok(())
}

/// One record's entity, binding and relationships join the retained import tx.
pub(super) fn import_legacy(
    tx: &Transaction<'_>,
    declarations: &Declarations,
    root: &str,
    id: &str,
) -> Result<()> {
    Catalog::new(declarations)?.mutate(
        tx,
        &Request::Register {
            root: root.into(),
            id: id.into(),
            request_id: uuid::Uuid::new_v4().to_string(),
        },
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
