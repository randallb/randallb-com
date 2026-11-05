//! Central declared persistence for the private Rust telemetry outbox.
//! The exporter owns privacy/configuration and HTTP. This module owns storage,
//! retained conversion and domain transactions; it grants no actor authority.
use crate::declared::{Catalog, EntityBinding as EntityCatalog};
use crate::{self as cwpdb, CwpdbResult};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::Path;

mod accounting;
#[cfg(test)]
mod accounting_tests;
mod admission;
#[cfg(test)]
mod reader_integration_tests;
mod reads;
#[cfg(test)]
mod startup_validation_tests;
pub use reads::PendingEvent;
mod migration;
mod mutations;
pub use mutations::Selection;
#[cfg(test)]
mod admission_tests;
#[cfg(test)]
mod architecture_tests;
#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod mutation_tests;
mod security;

pub const EVENT_DEFINITION: &str = "cwp://org.coworkerprotocol/cwr/entities/posthog-event";
pub const STATE_DEFINITION: &str = "cwp://org.coworkerprotocol/cwr/entities/posthog-state";
const STATE_REF: &str = "cwp://org.coworkerprotocol/cwr/posthog/state";
const MAX_QUEUE_EVENTS: usize = 10_000;
const MAX_QUEUE_BYTES: usize = 8 * 1024 * 1024;

struct Declarations {
    event: EntityCatalog,
    state: EntityCatalog,
}
impl Declarations {
    fn from_catalog(catalog: &Catalog) -> CwpdbResult<Self> {
        Ok(Self {
            event: catalog.entity(EVENT_DEFINITION)?,
            state: catalog.entity(STATE_DEFINITION)?,
        })
    }
}

/// Startup evidence, not a claim that any HTTP event was delivered.
#[derive(Debug, PartialEq, Eq)]
pub struct MigrationReport {
    pub imported: usize,
    pub high_water: i64,
    pub repeated: bool,
}

/// Owns the protected queue.sqlite3 connection. No raw SQL is exposed.
pub struct PosthogOutbox {
    db: Connection,
    declarations: Declarations,
    pub migration: MigrationReport,
}
impl PosthogOutbox {
    pub fn open(directory: &Path, catalog: &Catalog) -> CwpdbResult<Self> {
        let mut db = security::open(directory)?;
        let declarations = Declarations::from_catalog(catalog)?;
        let migration = migration::convert(&mut db, &declarations)?;
        Ok(Self {
            db,
            declarations,
            migration,
        })
    }

    pub fn installation_id(&self) -> CwpdbResult<String> {
        let state =
            self.declarations
                .state
                .get_in_connection(&self.db, STATE_DEFINITION, STATE_REF)?;
        Ok(state.value.properties["record"]["installation_id"]
            .as_str()
            .ok_or("invalid installation identity")?
            .into())
    }
}

fn event_ref(sequence: i64) -> String {
    format!("cwp://org.coworkerprotocol/cwr/posthog/events/{sequence}")
}
fn event_value(
    sequence: i64,
    received: i64,
    destination: String,
    event: String,
    quarantined: bool,
) -> Value {
    json!({"record":{"sequence":sequence,"received":received,"destination":destination,
        "bytes":event.len(),"event":event,"quarantined":quarantined}})
}

#[cfg(test)]
fn test_declarations() -> CwpdbResult<Declarations> {
    Declarations::from_catalog(&crate::desktop::test_catalog()?)
}
#[cfg(test)]
fn open(path: &Path) -> CwpdbResult<PosthogOutbox> {
    PosthogOutbox::open(path, &crate::desktop::test_catalog()?)
}
