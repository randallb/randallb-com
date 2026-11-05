//! Read-only legacy source; one graph destination transaction commits validated
//! instances, immutable versions, registry relationships and completion together.
use super::{registry, versions, OperationError, OperationErrorCode as Code};
use crate::{
    self as cwpdb, declared::Catalog as Declarations, ref_read::RefReadError, NativeStore,
};
use serde::Serialize;
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{fs, io::ErrorKind, path::Path};
const DEFINITION: &str = "cwp://org.coworkerprotocol/cwr/entities/desktop-import";
type Result<T> = std::result::Result<T, OperationError>;
fn error(code: Code) -> OperationError {
    OperationError { code }
}
fn storage(_: impl std::fmt::Display) -> OperationError {
    error(Code::StorageFailure)
}
#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Response {
    NoSource,
    AlreadyComplete,
    Complete { records: usize },
}

pub fn execute(
    store: &mut NativeStore,
    declarations: &Declarations,
    source: &str,
) -> Result<Response> {
    execute_with_probe(store, declarations, source, || {})
}
fn execute_with_probe(
    store: &mut NativeStore,
    declarations: &Declarations,
    source: &str,
    before_open: impl FnOnce(),
) -> Result<Response> {
    registry::validate_root(source)?;
    let reference = cwpdb::unordered::desktop_ref("storage-import", &json!(source));
    let catalog = declarations
        .entity("cwp://org.coworkerprotocol/cwr/entities/desktop-import")
        .map_err(storage)?;
    let completed = store
        .graph_read(|db| match cwpdb::ref_read::get_ref(db, &reference) {
            Err(RefReadError::NotFound) => Ok(false),
            Err(e) => Err(e.into()),
            Ok(_) => {
                let record = catalog.get_in_connection(db, DEFINITION, &reference)?;
                let mut value = record.value.properties;
                let format = value["record"]
                    .as_object_mut()
                    .ok_or_else(|| error(Code::UnsupportedDomainContract))?
                    .remove("format");
                if value != json!({"record":{"source":source,"complete":true}}) {
                    return Err(error(Code::UnsupportedDomainContract).into());
                }
                Ok(format == Some(json!(2)))
            }
        })
        .map_err(storage)?;
    if completed {
        return Ok(Response::AlreadyComplete);
    }
    // An initially absent source is not marked complete: a restored legacy
    // source remains eligible. A disappearance after this point is an error.
    let before = match fs::symlink_metadata(source) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Response::NoSource),
        Err(e) => return Err(storage(e)),
    };
    validate_source(source, &before)?;
    before_open();
    let mut legacy = NativeStore::open_read_only(source).map_err(storage)?;
    let stamp = legacy
        .graph_read(cwpdb::legacy_workspace::data_version)
        .map_err(storage)?;
    let data = legacy
        .graph_read(cwpdb::legacy_workspace::read)
        .map_err(
            |e| match e.downcast_ref::<cwpdb::legacy_workspace::Error>() {
                Some(cwpdb::legacy_workspace::Error::WorkLimit) => error(Code::ServiceLimit),
                Some(_) => error(Code::UnsupportedDomainContract),
                None => storage(e),
            },
        )?;
    let bytes = data
        .instances
        .iter()
        .map(|(root, id, name, folders)| {
            root.len() + id.len() + name.len() + folders.as_ref().map_or(0, String::len)
        })
        .sum::<usize>()
        + data
            .versions
            .iter()
            .map(|(root, id, files, folders)| root.len() + id.len() + files.len() + folders.len())
            .sum::<usize>();
    if bytes > 67_108_864 {
        return Err(error(Code::ServiceLimit));
    }
    let current = fs::symlink_metadata(source).map_err(storage)?;
    validate_source(source, &current)?;
    if !same_source(&before, &current) {
        return Err(error(Code::StorageFailure));
    }
    let records =
        data.instances.len() + data.versions.len() + data.active.len() + data.registry.len();
    store
        .graph_transaction(|tx| {
            // A peer may have completed while this source snapshot was loaded.
            if let Ok(record) = catalog.get_in_connection(tx, DEFINITION, &reference) {
                if record.value.properties
                    == json!({"record":{"source":source,"complete":true,"format":2}})
                {
                    return Ok(());
                }
                if record.value.properties != json!({"record":{"source":source,"complete":true}}) {
                    return Err(error(Code::UnsupportedDomainContract).into());
                }
            }
            versions::import_legacy(tx, declarations, &data)?;
            for (root, id) in &data.registry {
                registry::import_legacy(tx, declarations, root, id)?;
            }
            let after = fs::symlink_metadata(source)?;
            validate_source(source, &after)?;
            if !same_source(&before, &after)
                || legacy.graph_read(cwpdb::legacy_workspace::data_version)? != stamp
            {
                return Err(error(Code::StorageFailure).into());
            }
            let value = json!({"record":{"source":source,"complete":true,"format":2}});
            match cwpdb::ref_read::get_ref(tx, &reference) {
                Ok(before) => {
                    catalog.replace_in_transaction(
                        tx,
                        DEFINITION,
                        &reference,
                        &before.revision,
                        &value,
                    )?;
                }
                Err(RefReadError::NotFound) => {
                    catalog.create_in_transaction(tx, DEFINITION, &reference, &value)?;
                }
                Err(e) => return Err(e.into()),
            }
            Ok(())
        })
        .map_err(|e| match e.downcast::<OperationError>() {
            Ok(e) => *e,
            Err(_) => error(Code::StorageFailure),
        })?;
    Ok(Response::Complete { records })
}
fn validate_source(source: &str, metadata: &fs::Metadata) -> Result<()> {
    if !Path::new(source).is_absolute() || !metadata.is_file() || metadata.file_type().is_symlink()
    {
        return Err(error(Code::InvalidInput));
    }
    if metadata.len() > 134_217_728 {
        return Err(error(Code::ServiceLimit));
    }
    if let Ok(wal) = fs::symlink_metadata(format!("{source}-wal")) {
        if wal.file_type().is_symlink() || wal.len() > 134_217_728 {
            return Err(error(Code::ServiceLimit));
        }
    }
    Ok(())
}
fn same_source(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    #[cfg(unix)]
    if before.dev() != after.dev() || before.ino() != after.ino() {
        return false;
    }
    before.len() == after.len() && before.modified().ok() == after.modified().ok()
}

#[cfg(test)]
#[path = "workspace_import_tests.rs"]
mod tests;
