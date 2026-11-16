//! Named storage operations for retained workspace environment metadata.
//! Secret values and external credential effects never enter this store.
use crate::{CwpdbResult, NativeStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::Path, sync::Mutex};
#[cfg(feature = "test-support")]
pub mod fixtures;
mod mutations;
mod records;
mod validation;

pub const WORKSPACE_KIND: &str = "wlp_workspace";
pub const ENTRY_KIND: &str = "wlp_environment_entry";
pub const CLEANUP_KIND: &str = "wlp_credential_cleanup";
pub const MEMBERSHIP_KIND: &str = "environment_entry";
pub const CLEANUP_EDGE_KIND: &str = "credential_cleanup";
pub const WORKSPACE_SCHEMA: &str = "cwp://org.coworkerprotocol/wlp/schemas/workspace/2026-09";
pub const ENTRY_SCHEMA: &str = "cwp://org.coworkerprotocol/wlp/schemas/environment_entry/2026-09";
pub const CLEANUP_SCHEMA: &str =
    "cwp://org.coworkerprotocol/wlp/schemas/credential_cleanup/2026-09";
const STORAGE_REF: &str = "cwp://org.coworkerprotocol.local/wlp/storage";
const STORAGE_KIND: &str = "wlp_storage";
const STORAGE_SCHEMA: &str = "cwp://org.coworkerprotocol/wlp/schemas/storage/2026-09";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EntryValue {
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub reference: String,
    pub revision: i64,
    pub value: EntryValue,
}
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub cleanup_pending: bool,
}
pub struct Cleanup {
    pub reference: String,
    pub credential_id: String,
    pub in_use: bool,
}
pub struct Save<'a> {
    pub workspace: &'a str,
    pub reference: &'a str,
    pub expected_revision: Option<i64>,
    pub value: &'a EntryValue,
}
/// One native metadata owner. Methods serialize named read/check/commit work;
/// the caller cannot access SQLite, choose indexes, or compose arbitrary SQL.
pub struct EnvironmentStore {
    store: Mutex<NativeStore>,
}
impl EnvironmentStore {
    pub fn exists(directory: &Path) -> bool {
        directory.join("wlpdb.sqlite3").exists()
    }
    pub fn open(directory: &Path, identity: &str) -> CwpdbResult<Self> {
        let store = Self {
            store: Mutex::new(NativeStore::open(directory.join("wlpdb.sqlite3"))?),
        };
        store.initialize(identity)?;
        Ok(store)
    }
    pub fn in_memory(identity: &str) -> CwpdbResult<Self> {
        let store = Self {
            store: Mutex::new(NativeStore::open_in_memory()?),
        };
        store.initialize(identity)?;
        Ok(store)
    }
    fn initialize(&self, identity: &str) -> CwpdbResult<()> {
        self.store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_transaction(|tx| {
                validation::validate_formats(tx)?;
                match crate::ref_read::get_ref(tx, STORAGE_REF) {
                    Ok(record) => {
                        if record.kind != STORAGE_KIND
                            || record.value.schema_ref != STORAGE_SCHEMA
                            || record
                                .value
                                .properties
                                .pointer("/record/storage_identity")
                                .and_then(Value::as_str)
                                != Some(identity)
                        {
                            return Err(
                                "WLP DB belongs to a different Workloop storage identity.".into()
                            );
                        }
                    }
                    Err(crate::ref_read::RefReadError::NotFound) => {
                        crate::put_node_with_schema(
                            tx,
                            STORAGE_REF,
                            STORAGE_KIND,
                            STORAGE_SCHEMA,
                            &serde_json::json!({"record":{"storage_identity":identity}}),
                        )?;
                    }
                    Err(error) => return Err(error.into()),
                }
                Ok(())
            })
    }
    pub fn bind_workspace(&self, reference: &str, path_hash: &str) -> CwpdbResult<()> {
        self.store
            .lock()
            .map_err(|_| "environment store lock poisoned")?
            .graph_transaction(|tx| {
                match crate::ref_read::get_ref(tx, reference) {
                    Ok(record) => {
                        if record.kind != WORKSPACE_KIND
                            || record.value.schema_ref != WORKSPACE_SCHEMA
                            || record
                                .value
                                .properties
                                .pointer("/record/path_hash")
                                .and_then(Value::as_str)
                                != Some(path_hash)
                        {
                            return Err("WLP DB has an unsupported workspace record.".into());
                        }
                    }
                    Err(crate::ref_read::RefReadError::NotFound) => {
                        crate::put_node_with_schema(
                            tx,
                            reference,
                            WORKSPACE_KIND,
                            WORKSPACE_SCHEMA,
                            &serde_json::json!({"record":{"path_hash":path_hash}}),
                        )?;
                    }
                    Err(error) => return Err(error.into()),
                }
                Ok(())
            })
    }
}
