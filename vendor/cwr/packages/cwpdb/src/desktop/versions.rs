//! Immutable version arrays own grants. Membership is a recoverable projection;
//! publication/readiness and active selection/projection each commit atomically.
use super::{
    receipts::{Receipts, Replay},
    registry::validate_root,
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
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
#[path = "versions_mutations.rs"]
mod mutations;
const INSTANCE: &str = "cwp://org.coworkerprotocol/cwr/entities/desktop-instance";
const VERSION: &str = "cwp://org.coworkerprotocol/cwr/entities/desktop-version";
const GRANT: &str = "cwp://org.coworkerprotocol/cwr/entities/desktop-folder";
const READY: &str = "cwp://org.coworkerprotocol/cwr/entities/desktop-version-membership";
const INSTANCE_KIND: &str = "desktop_workspace_instance_v1";
const VERSION_KIND: &str = "desktop_workspace_version_v1";
const GRANT_KIND: &str = "desktop_workspace_folder_v1";
const MAX_WORK: usize = 10_000;
const VERSION_EDGE: Contract = Contract {
    kind: VERSION_KIND,
    schema: "cwp://org.coworkerprotocol/cwpdb/schemas/desktop_workspace_version_v1/2026-09",
    source_kind: INSTANCE_KIND,
    target_kind: VERSION_KIND,
    external_target: false,
    source_cardinality: Cardinality::Many,
    target_cardinality: Cardinality::One,
};
const ACTIVE_EDGE: Contract = Contract {
    kind: "desktop_active_version_v1",
    schema: "cwp://org.coworkerprotocol/cwpdb/schemas/desktop_active_version_v1/2026-09",
    source_kind: INSTANCE_KIND,
    target_kind: VERSION_KIND,
    external_target: false,
    source_cardinality: Cardinality::One,
    target_cardinality: Cardinality::One,
};
const MEMBER_EDGE: Contract = Contract {
    kind: "desktop_version_folder_v1",
    schema: "cwp://org.coworkerprotocol/cwpdb/schemas/desktop_version_folder_v1/2026-10-23",
    source_kind: VERSION_KIND,
    target_kind: GRANT_KIND,
    external_target: false,
    source_cardinality: Cardinality::Many,
    target_cardinality: Cardinality::Many,
};
const GRANT_EDGE: Contract = Contract {
    kind: "desktop_instance_folder_v1",
    schema: "cwp://org.coworkerprotocol/cwpdb/schemas/desktop_instance_folder_v1/2026-10-23",
    source_kind: INSTANCE_KIND,
    target_kind: GRANT_KIND,
    external_target: false,
    source_cardinality: Cardinality::Many,
    target_cardinality: Cardinality::Many,
};
type Result<T> = std::result::Result<T, OperationError>;
fn error(code: Code) -> OperationError {
    OperationError { code }
}
fn storage(_: impl std::fmt::Display) -> OperationError {
    error(Code::StorageFailure)
}
fn relation(e: unordered::Error) -> OperationError {
    error(match e {
        unordered::Error::WorkLimit => Code::ServiceLimit,
        unordered::Error::RelationshipsExist => Code::RelationshipsExist,
        unordered::Error::RevisionConflict => Code::RevisionConflict,
        unordered::Error::Storage(_) => Code::StorageFailure,
        _ => Code::UnsupportedDomainContract,
    })
}
#[path = "versions_types.rs"]
mod types;
pub use types::*;
struct Catalog {
    instance: EntityCatalog,
    version: EntityCatalog,
    grant: EntityCatalog,
    ready: EntityCatalog,
    receipts: Receipts,
}
impl Catalog {
    fn new(declarations: &Declarations) -> Result<Self> {
        Ok(Self {
            instance: declarations
                .entity("cwp://org.coworkerprotocol/cwr/entities/desktop-instance")
                .map_err(storage)?,
            version: declarations
                .entity("cwp://org.coworkerprotocol/cwr/entities/desktop-version")
                .map_err(storage)?,
            grant: declarations
                .entity("cwp://org.coworkerprotocol/cwr/entities/desktop-folder")
                .map_err(storage)?,
            ready: declarations
                .entity("cwp://org.coworkerprotocol/cwr/entities/desktop-version-membership")
                .map_err(storage)?,
            receipts: Receipts::new(declarations, INSTANCE_KIND)?,
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
                    INSTANCE => &self.instance,
                    VERSION => &self.version,
                    GRANT => &self.grant,
                    READY => &self.ready,
                    _ => return Err(error(Code::UnsupportedDomainContract)),
                };
                catalog
                    .get_in_connection(db, definition, reference)
                    .map(Some)
                    .map_err(|_| error(Code::UnsupportedDomainContract))
            }
        }
    }
    fn instance(&self, db: &Connection, root: &str) -> Result<Option<Instance>> {
        let Some(record) = self.read(db, INSTANCE, &instance_ref(root))? else {
            return Ok(None);
        };
        let value = &record.value.properties["record"];
        if value["removed"] == true {
            return Ok(None);
        }
        if value["root"] != root {
            return Err(error(Code::UnsupportedDomainContract));
        }
        let mut value = value.clone();
        value
            .as_object_mut()
            .ok_or_else(|| error(Code::UnsupportedDomainContract))?
            .remove("root");
        value
            .as_object_mut()
            .ok_or_else(|| error(Code::UnsupportedDomainContract))?
            .remove("removed");
        serde_json::from_value(value)
            .map(Some)
            .map_err(|_| error(Code::UnsupportedDomainContract))
    }
    fn version(&self, db: &Connection, root: &str, id: &str) -> Result<Option<Version>> {
        let Some(record) = self.read(db, VERSION, &version_ref(root, id))? else {
            return Ok(None);
        };
        let version: Version = serde_json::from_value(record.value.properties["record"].clone())
            .map_err(|_| error(Code::UnsupportedDomainContract))?;
        if version.id != id {
            return Err(error(Code::UnsupportedDomainContract));
        }
        Ok(Some(version))
    }
    fn active(&self, db: &Connection, root: &str) -> Result<Option<Version>> {
        let Some(edge) =
            unordered::single_outgoing(db, ACTIVE_EDGE, &instance_ref(root)).map_err(relation)?
        else {
            return Ok(None);
        };
        let record = self
            .read(db, VERSION, &edge.target)?
            .ok_or_else(|| error(Code::UnsupportedDomainContract))?;
        let version: Version = serde_json::from_value(record.value.properties["record"].clone())
            .map_err(|_| error(Code::UnsupportedDomainContract))?;
        if edge.target != version_ref(root, &version.id) {
            return Err(error(Code::UnsupportedDomainContract));
        }
        Ok(Some(version))
    }
    fn ready(
        &self,
        db: &Connection,
        root: &str,
        instance: &Instance,
        version: &Version,
    ) -> Result<bool> {
        let Some(record) = self.read(db, READY, &ready_ref(root, &version.id))? else {
            return Ok(false);
        };
        Ok(record.value.properties == ready_value(instance, version))
    }
    fn snapshot(&self, db: &Connection, root: &str) -> Result<Response> {
        let revision = self
            .read(db, INSTANCE, &instance_ref(root))?
            .map(|record| record.revision.legacy_counter().to_string());
        let Some(mut instance) = self.instance(db, root)? else {
            return Ok(Response::Instance {
                instance: None,
                revision,
                membership_ready: true,
            });
        };
        let mut ready = true;
        if let Some(version) = self.active(db, root)? {
            ready = self.ready(db, root, &instance, &version)?;
            instance.folders = Some(version.folders);
        }
        Ok(Response::Instance {
            instance: Some(instance),
            revision,
            membership_ready: ready,
        })
    }
    fn version_snapshot(&self, db: &Connection, root: &str, id: Option<&str>) -> Result<Response> {
        let version = match id {
            Some(id) => self.version(db, root, id)?,
            None => self.active(db, root)?,
        };
        let ready = match (&version, self.instance(db, root)?) {
            (Some(version), Some(instance)) => self.ready(db, root, &instance, version)?,
            (Some(_), None) => false,
            _ => true,
        };
        Ok(Response::Version {
            version,
            membership_ready: ready,
        })
    }
    fn save_instance_value(
        &self,
        tx: &Transaction<'_>,
        root: &str,
        instance: &Instance,
    ) -> Result<()> {
        let reference = instance_ref(root);
        let value = json!({"record":{"root":root,"id":instance.id,"name":instance.name,"folders":instance.folders}});
        if let Some(before) = self.read(tx, INSTANCE, &reference)? {
            unordered::adopt_entity(tx, &reference, INSTANCE_KIND).map_err(storage)?;
            if before.value.properties != value {
                self.instance
                    .replace_in_transaction(tx, INSTANCE, &reference, &before.revision, &value)
                    .map_err(storage)?;
            }
        } else {
            if unordered::retired(tx, &reference).map_err(storage)? {
                return Err(error(Code::RefRetired));
            }
            self.instance
                .create_in_transaction(tx, INSTANCE, &reference, &value)
                .map_err(storage)?;
        }
        Ok(())
    }
    fn membership(&self, tx: &Transaction<'_>, root: &str, version: &Version) -> Result<()> {
        let instance = self
            .instance(tx, root)?
            .ok_or_else(|| error(Code::NotFound))?;
        if self.ready(tx, root, &instance, version)? {
            return Ok(());
        }
        if version.folders.len() > MAX_WORK {
            return Err(error(Code::ServiceLimit));
        }
        let owner = instance_ref(root);
        let reference = version_ref(root, &version.id);
        unordered::adopt_entity(tx, &reference, VERSION_KIND).map_err(storage)?;
        for edge in
            unordered::many_outgoing(tx, MEMBER_EDGE, &reference, MAX_WORK).map_err(relation)?
        {
            unordered::unlink(tx, MEMBER_EDGE, &reference, &edge.target, edge.revision)
                .map_err(relation)?;
        }
        for path in version.folders.iter().collect::<BTreeSet<_>>() {
            if path.is_empty() || path.len() > 16_384 || path.contains('\0') {
                return Err(error(Code::InvalidValue));
            }
            let grant = unordered::desktop_ref("workspace-folder", &json!([instance.id, path]));
            let value = json!({"record":{"workspace":instance.id,"path":path}});
            if let Some(before) = self.read(tx, GRANT, &grant)? {
                if before.value.properties != value {
                    return Err(error(Code::UnsupportedDomainContract));
                }
            } else {
                if unordered::retired(tx, &grant).map_err(storage)? {
                    return Err(error(Code::RefRetired));
                }
                self.grant
                    .create_in_transaction(tx, GRANT, &grant, &value)
                    .map_err(storage)?;
            }
            unordered::link(tx, MEMBER_EDGE, &reference, &grant).map_err(relation)?;
            unordered::link(tx, GRANT_EDGE, &owner, &grant).map_err(relation)?;
        }
        unordered::link(tx, VERSION_EDGE, &owner, &reference).map_err(relation)?;
        let ready = ready_ref(root, &version.id);
        let value = ready_value(&instance, version);
        if let Some(before) = self.read(tx, READY, &ready)? {
            self.ready
                .replace_in_transaction(tx, READY, &ready, &before.revision, &value)
                .map_err(storage)?;
        } else {
            self.ready
                .create_in_transaction(tx, READY, &ready, &value)
                .map_err(storage)?;
        }
        Ok(())
    }
    fn selection(
        &self,
        tx: &Transaction<'_>,
        root: &str,
        id: &str,
        expected: Option<&str>,
        policy: SelectionPolicy,
    ) -> Result<()> {
        let current = self.active(tx, root)?;
        if matches!(policy, SelectionPolicy::IfAbsent) && current.is_some() {
            return Ok(());
        }
        if current.as_ref().map(|version| version.id.as_str()) != expected {
            return Err(error(Code::RevisionConflict));
        }
        let version = self
            .version(tx, root, id)?
            .ok_or_else(|| error(Code::NotFound))?;
        self.membership(tx, root, &version)?;
        let owner = instance_ref(root);
        if let Some(edge) = unordered::single_outgoing(tx, ACTIVE_EDGE, &owner).map_err(relation)? {
            if edge.target != version_ref(root, id) {
                unordered::unlink(tx, ACTIVE_EDGE, &owner, &edge.target, edge.revision)
                    .map_err(relation)?;
            }
        }
        let mut instance = self
            .instance(tx, root)?
            .ok_or_else(|| error(Code::NotFound))?;
        instance.folders = Some(version.folders);
        self.save_instance_value(tx, root, &instance)?;
        unordered::link(tx, ACTIVE_EDGE, &owner, &version_ref(root, id)).map_err(relation)?;
        Ok(())
    }
}
fn ready_value(instance: &Instance, version: &Version) -> Value {
    json!({"record":{"ready":true,"workspace":instance.id,"fingerprint":hex::encode(Sha256::digest(serde_json::to_vec(&version.folders).expect("folders serialize"))),"count":version.folders.iter().collect::<BTreeSet<_>>().len()}})
}
fn instance_ref(root: &str) -> String {
    unordered::desktop_ref("workspace-instance", &json!(root))
}
fn version_ref(root: &str, id: &str) -> String {
    unordered::desktop_ref("workspace-version", &json!([root, id]))
}
fn ready_ref(root: &str, id: &str) -> String {
    unordered::desktop_ref("version-membership", &json!(version_ref(root, id)))
}

pub fn execute(
    store: &mut NativeStore,
    declarations: &Declarations,
    request: Request,
) -> Result<Response> {
    validate_root(request.root())?;
    let catalog = Catalog::new(declarations)?;
    let result = match &request {
        Request::ReadInstance { root } => store.graph_read(|db| Ok(catalog.snapshot(db, root)?)),
        Request::ReadVersion { root, version } => {
            store.graph_read(|db| Ok(catalog.version_snapshot(db, root, version.as_deref())?))
        }
        Request::Recover { root, version } => store.graph_transaction(|tx| {
            let target = match version {
                Some(id) => catalog.version(tx, root, id)?,
                None => catalog.active(tx, root)?,
            };
            if let Some(target) = target {
                catalog.membership(tx, root, &target)?;
                if catalog
                    .active(tx, root)?
                    .is_some_and(|active| active.id == target.id)
                {
                    let mut instance = catalog
                        .instance(tx, root)?
                        .ok_or_else(|| error(Code::NotFound))?;
                    instance.folders = Some(target.folders);
                    catalog.save_instance_value(tx, root, &instance)?;
                }
            }
            Ok(Response::Committed)
        }),
        _ => store.graph_transaction(|tx| Ok(catalog.mutate(tx, &request)?)),
    };
    result.map_err(|e| match e.downcast::<OperationError>() {
        Ok(e) => *e,
        Err(_) => error(Code::StorageFailure),
    })
}

/// All retained workspace versions, selections and membership changes join the
/// caller's one destination import transaction. No incomplete import marker.
pub(super) fn import_legacy(
    tx: &Transaction<'_>,
    declarations: &Declarations,
    data: &cwpdb::legacy_workspace::Data,
) -> Result<()> {
    let catalog = Catalog::new(declarations)?;
    for (root, id, name, folders) in &data.instances {
        validate_root(root)?;
        let folders = folders
            .as_ref()
            .map(|value| serde_json::from_str::<Vec<String>>(value))
            .transpose()
            .map_err(|_| error(Code::InvalidValue))?;
        let instance = Instance {
            id: id.clone(),
            name: name.clone(),
            folders,
        };
        if let Some(existing) = catalog.instance(tx, root)? {
            if existing.id != instance.id {
                return Err(error(Code::UnsupportedDomainContract));
            }
        } else {
            if catalog.read(tx, INSTANCE, &instance_ref(root))?.is_some() {
                return Err(error(Code::UnsupportedDomainContract));
            }
            catalog.save_instance_value(tx, root, &instance)?;
        }
    }
    for (root, id, files, folders) in &data.versions {
        validate_root(root)?;
        catalog
            .instance(tx, root)?
            .ok_or_else(|| error(Code::UnsupportedDomainContract))?;
        let version = Version {
            id: id.clone(),
            files: serde_json::from_str(files).map_err(|_| error(Code::InvalidValue))?,
            folders: serde_json::from_str(folders).map_err(|_| error(Code::InvalidValue))?,
        };
        catalog.publish(tx, root, &version, true)?;
    }
    for (root, id) in &data.active {
        validate_root(root)?;
        catalog.selection(tx, root, id, None, SelectionPolicy::IfAbsent)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "versions_tests.rs"]
mod tests;
