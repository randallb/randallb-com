//! Typed retained-state fixtures for downstream compatibility tests.
//! Enabled only by the explicit test-support development feature.
use crate::CwpdbResult;
use std::path::Path;

/// Construct one fictional pre-graph telemetry outbox for migration tests.
/// This grants no arbitrary SQL interface and returns no native connection.
pub fn legacy_posthog_outbox(
    path: &Path,
    destination: &str,
    received_at_ms: u64,
    event_json: &str,
) -> CwpdbResult<()> {
    let event: serde_json::Value = serde_json::from_str(event_json)?;
    if !event.is_object() || destination.is_empty() {
        return Err("invalid legacy outbox fixture".into());
    }
    let db = rusqlite::Connection::open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    db.execute_batch("CREATE TABLE queue(id INTEGER PRIMARY KEY AUTOINCREMENT,destination TEXT NOT NULL,received INTEGER NOT NULL,event TEXT NOT NULL,quarantined INTEGER NOT NULL DEFAULT 0);CREATE TABLE identity(value TEXT PRIMARY KEY);CREATE TABLE counters(key TEXT PRIMARY KEY,value INTEGER NOT NULL);INSERT INTO identity VALUES('retained-installation');INSERT INTO counters VALUES('dropped',7);")?;
    db.execute(
        "INSERT INTO queue VALUES(99,?1,?2,?3,0)",
        rusqlite::params![destination, i64::try_from(received_at_ms)?, event_json],
    )?;
    Ok(())
}

/// Seed an exact retained revision for numeric precision compatibility tests.
/// Only changes the counter of an existing entity; no SQL or values cross this seam.
pub fn entity_revision(
    store: &mut crate::NativeStore,
    reference: &str,
    revision: i64,
) -> CwpdbResult<()> {
    if revision < 1 {
        return Err("invalid fixture revision".into());
    }
    store.graph_transaction(|tx| {
        crate::ref_read::get_ref(tx, reference)?;
        let changed = tx.execute("UPDATE nodes SET revision=?1 WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)", rusqlite::params![revision, reference])?;
        if changed != 1 { return Err("fixture entity missing".into()); }
        Ok(())
    })
}

pub mod conversation;

/// Completion-write failure fixtures.
pub mod completion;

pub mod retained_relationships;

pub mod conversation_observations;
pub mod publication_faults;
pub mod tool_files;

pub mod commit_faults;

pub mod conversation_claims;

pub mod invocation_order;

pub mod message_boundaries;

pub mod summary_windows;

pub mod summary_pages;

pub mod indexed_history;

pub mod timeline_persistence;

pub mod invocation_reads;

pub mod admission_history;

pub mod legacy_invocation;

pub mod executor_start_receipt;
