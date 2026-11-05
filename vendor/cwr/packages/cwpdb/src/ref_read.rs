//! Authoritative resource reads for already-authorized, canonical refs.

use crate::CwpdbResult;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An opaque comparison token for one resource record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefRevision(i64);

impl RefRevision {
    pub(crate) fn from_storage_counter(value: i64) -> Self {
        Self(value)
    }
    /// Transitional adapter for existing domain APIs that already expose the
    /// SQLite revision counter. New callers should compare opaque tokens.
    pub fn legacy_counter(&self) -> i64 {
        self.0
    }
}

/// The schema and complete stored JSON object owned by the resource domain.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RefValue {
    pub schema_ref: String,
    pub properties: Value,
}

/// A resource read without physical IDs or storage keys.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RefRecord {
    pub reference: String,
    /// Internal domain discriminator, retained for existing schema validation.
    pub kind: String,
    pub value: RefValue,
    pub revision: RefRevision,
}

#[derive(Debug)]
pub enum RefReadError {
    NotFound,
    WrongRecordType,
    Storage(Box<dyn std::error::Error + Send + Sync>),
}

impl std::fmt::Display for RefReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("resource not found"),
            Self::WrongRecordType => f.write_str("reference identifies an edge"),
            Self::Storage(error) => write!(f, "resource read failed: {error}"),
        }
    }
}

impl std::error::Error for RefReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

/// Read a resource directly from authoritative graph state in one SQLite
/// statement/snapshot. A binding without a record is not a resource.
///
/// This is the storage implementation of getRef, not a public authorization
/// boundary: the domain must authorize the canonical ref before calling and
/// validate its schema before using its value. Use an autocommit connection
/// (or a fresh read transaction) for visibility of already-committed writes;
/// an existing caller-owned snapshot intentionally retains its older view.
pub fn get_ref(db: &Connection, reference: &str) -> Result<RefRecord, RefReadError> {
    let read = || -> CwpdbResult<_> {
        Ok(db
            .prepare_cached(
                "SELECT n.kind,s.canonical_ref,json(n.props),n.revision,
                    EXISTS(SELECT 1 FROM edges e WHERE e.ref_id=b.ref_id)
             FROM ref_bindings b
             LEFT JOIN nodes n ON n.ref_id=b.ref_id
             LEFT JOIN ref_bindings s ON s.ref_id=n.schema_id
             WHERE b.canonical_ref=?1",
            )?
            .query_row([reference], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            })
            .optional()?)
    };
    let Some((kind, schema, properties, revision, edge)) = read().map_err(RefReadError::Storage)?
    else {
        return Err(RefReadError::NotFound);
    };
    if edge {
        return Err(RefReadError::WrongRecordType);
    }
    let Some(kind) = kind else {
        return Err(RefReadError::NotFound);
    };
    let (Some(schema_ref), Some(properties), Some(revision)) = (schema, properties, revision)
    else {
        return Err(RefReadError::Storage("incomplete resource record".into()));
    };
    Ok(RefRecord {
        reference: reference.to_owned(),
        kind,
        value: RefValue {
            schema_ref,
            properties: serde_json::from_str(&properties)
                .map_err(|error| RefReadError::Storage(error.into()))?,
        },
        revision: RefRevision(revision),
    })
}

#[cfg(test)]
#[path = "ref_read_tests.rs"]
mod tests;
