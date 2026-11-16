use super::*;
use crate::CwpdbResult;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
mod connection;
use connection::NativeConnection;
mod history;
mod history_read;
mod initialization;
mod maintenance;
mod pages;
mod read;
mod read_snapshot;
mod receipts;
mod resource;
mod timeline_read;
pub use read_snapshot::{NativeReadSnapshot, NativeReadStore};
mod summary;
#[cfg(feature = "test-support")]
mod test_audit;
mod write;

/// A retained native CWR graph; database mechanics never leave cwpdb.
pub struct NativeConversationStore {
    connection: NativeConnection,
    scope: Scope,
    identity: Option<FileIdentity>,
    cursors: BTreeMap<String, read::Cursor>,
    next_cursor: u64,
}
#[derive(Clone)]
struct FileIdentity {
    path: PathBuf,
    device: u64,
    inode: u64,
}
impl NativeConversationStore {
    pub fn open(path: &Path, scope: Scope, options: OpenOptions) -> CwpdbResult<Self> {
        let connection = initialization::open(path, &scope, &options)?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(path)?;
            Some(FileIdentity {
                path: path.into(),
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        };
        #[cfg(not(unix))]
        let identity = None;
        Ok(Self {
            connection: NativeConnection::new(connection),
            scope,
            identity,
            cursors: BTreeMap::new(),
            next_cursor: 0,
        })
    }
    pub fn in_memory(scope: Scope, options: OpenOptions) -> CwpdbResult<Self> {
        let mut connection = Connection::open_in_memory()?;
        let tx = connection.transaction()?;
        crate::initialize(&tx)?;
        initialization::initialize(&tx, &scope, &options)?;
        tx.commit()?;
        Ok(Self {
            connection: NativeConnection::new(connection),
            scope,
            identity: None,
            cursors: BTreeMap::new(),
            next_cursor: 0,
        })
    }
    fn check_scope(&self, scope: &Scope) -> CwpdbResult<()> {
        if scope.runtime_root_ref != self.scope.runtime_root_ref
            || scope.context.workspace_ref != self.scope.context.workspace_ref
        {
            return Err("conversation storage scope mismatch".into());
        }
        #[cfg(unix)]
        if let Some(identity) = &self.identity {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(&identity.path)?;
            if !metadata.is_file()
                || metadata.dev() != identity.device
                || metadata.ino() != identity.inode
            {
                return Err("CWR runtime database was replaced while open".into());
            }
        }
        Ok(())
    }
}
fn root_ref(scope: &Scope) -> String {
    format!("{}/cwpdb", scope.runtime_root_ref)
}
fn revision(scope: &Scope, counter: i64) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(
        json!([root_ref(scope), counter]).to_string().as_bytes(),
    ))
}
fn runtime_metadata(db: &Connection, scope: &Scope) -> CwpdbResult<Value> {
    crate::node_props(db, &root_ref(scope))?.ok_or_else(|| "missing CWR runtime metadata".into())
}
fn current_revision(db: &Connection, scope: &Scope) -> CwpdbResult<(String, i64)> {
    let counter = runtime_metadata(db, scope)?["commit_revision"]
        .as_i64()
        .filter(|counter| *counter >= 0)
        .ok_or("invalid CWR commit revision")?;
    Ok((revision(scope, counter), counter))
}
fn ref_revision(reference: &str, counter: i64) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(
        json!([reference, counter]).to_string().as_bytes(),
    ))
}
pub(super) fn valid_ref(reference: &str) -> CwpdbResult<()> {
    if !reference.starts_with("cwp://")
        || reference.len() <= 6
        || reference.len() > 4096
        || reference.contains('#')
        || reference.chars().any(char::is_whitespace)
    {
        return Err("invalid canonical CWR ref".into());
    }
    Ok(())
}
fn parse_kind(kind: &str) -> CwpdbResult<RecordKind> {
    Ok(serde_json::from_value(Value::String(kind.into()))?)
}

#[cfg(test)]
mod retained_schema_tests;

#[cfg(test)]
mod relationship_fixture_tests;

#[cfg(all(test, unix))]
mod identity_fence_tests;

#[cfg(all(test, unix))]
mod private_schema_tests;
