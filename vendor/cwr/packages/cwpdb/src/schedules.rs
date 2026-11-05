//! Named installed-schedule persistence. CWR owns timing, exact action validation
//! and normal admission; this backend owns identities, revisions and atomic links.
use crate::{declared::Catalog, ref_read::RefRevision, CwpdbResult, NativeStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod native;
mod remote;
pub use remote::RemoteSchedules;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    pub schedule_ref: String,
    pub owner_context_ref: String,
    pub author_actor_ref: String,
    pub definition: Value,
    pub availability: String,
    pub paused: bool,
    pub next_due_unix_ms: u64,
    pub active_occurrence_ref: Option<String>,
    pub last_admission_error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Occurrence {
    pub occurrence_ref: String,
    pub schedule_ref: String,
    pub definition: Value,
    pub due_unix_ms: u64,
    pub outcome: String,
    pub invocation_ref: Option<String>,
    pub conversation_ref: Option<String>,
    pub archived: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Versioned<T> {
    pub value: T,
    pub revision: RefRevision,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum OccurrenceSelection {
    Recent(usize),
    UnarchivedTerminal,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Install {
    pub runtime_root_ref: String,
    pub owner_context_ref: String,
    pub author_actor_ref: String,
    pub definition: Value,
    pub next_due_unix_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Advance {
    pub schedule_ref: String,
    pub expected: RefRevision,
    pub next_due_unix_ms: u64,
    pub active_occurrence_ref: Option<String>,
    pub last_admission_error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settle {
    pub occurrence_ref: String,
    pub expected: RefRevision,
    pub outcome: String,
    pub invocation_ref: Option<String>,
    pub conversation_ref: Option<String>,
    pub archived: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reservation {
    pub schedule_ref: String,
    pub expected: RefRevision,
    pub occurrence: Occurrence,
    pub next_due_unix_ms: u64,
    pub active_occurrence_ref: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Availability {
    pub schedule_ref: String,
    pub expected: RefRevision,
    pub availability: String,
    pub next_due_unix_ms: u64,
}
/// Each operation is complete; no physical graph callback or handle is exposed.
pub trait ScheduleStorage {
    #[cfg(feature = "test-support")]
    fn seed_and_verify_history_fixture(
        &mut self,
        _schedule: &Installed,
        _count: usize,
    ) -> CwpdbResult<String> {
        Err("native schedule history fixture is unavailable for this backend".into())
    }

    fn installed(
        &mut self,
        catalog: &Catalog,
        runtime_root_ref: &str,
        owner: Option<&str>,
    ) -> CwpdbResult<Vec<Versioned<Installed>>>;
    fn get_schedule(
        &mut self,
        catalog: &Catalog,
        reference: &str,
        owner: Option<&str>,
    ) -> CwpdbResult<Option<Versioned<Installed>>>;
    fn get_occurrence(
        &mut self,
        catalog: &Catalog,
        reference: &str,
    ) -> CwpdbResult<Option<Versioned<Occurrence>>>;
    fn occurrences(
        &mut self,
        catalog: &Catalog,
        schedule_ref: &str,
        selection: OccurrenceSelection,
    ) -> CwpdbResult<Vec<Versioned<Occurrence>>>;
    fn archive_candidates(
        &mut self,
        catalog: &Catalog,
        runtime_root_ref: &str,
    ) -> CwpdbResult<Vec<(String, Versioned<Occurrence>)>>;
    fn install(
        &mut self,
        catalog: &Catalog,
        request: &Install,
    ) -> CwpdbResult<Versioned<Installed>>;
    fn pause(
        &mut self,
        catalog: &Catalog,
        owner: &str,
        reference: &str,
        expected: &RefRevision,
        paused: bool,
        next_due: u64,
    ) -> CwpdbResult<Versioned<Installed>>;
    fn availability(
        &mut self,
        catalog: &Catalog,
        owner: &str,
        changes: &[Availability],
    ) -> CwpdbResult<()>;
    fn advance(
        &mut self,
        catalog: &Catalog,
        request: &Advance,
    ) -> CwpdbResult<Versioned<Installed>>;
    fn reserve(
        &mut self,
        catalog: &Catalog,
        request: &Reservation,
    ) -> CwpdbResult<Versioned<Installed>>;
    fn settle(&mut self, catalog: &Catalog, request: &Settle)
        -> CwpdbResult<Versioned<Occurrence>>;
}
fn identity(kind: &str, value: impl Serialize) -> CwpdbResult<String> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "cwp://org.coworkerprotocol.local/{kind}/{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&value)?))
    ))
}
pub fn owner_binding(owner: &str) -> CwpdbResult<String> {
    identity("installed-schedule-owners", owner)
}
const SCHEDULE: &str = "cwp://org.coworkerprotocol/cwr/entities/installed-schedule";
const OCCURRENCE: &str = "cwp://org.coworkerprotocol/cwr/entities/schedule-occurrence";
const OWNER: &str = "cwp://org.coworkerprotocol/cwr/entities/schedule-owner";

fn time(value: u64) -> CwpdbResult<()> {
    if value > 9_007_199_254_740_991 {
        return Err("schedule time exceeds safe millisecond range".into());
    }
    Ok(())
}
fn request_budget(value: &impl Serialize) -> CwpdbResult<()> {
    if serde_json::to_vec(value)?.len() > 512 * 1024 {
        return Err("schedule request exceeds512KiB".into());
    }
    Ok(())
}
fn response_budget<T: Serialize>(value: T) -> CwpdbResult<T> {
    if serde_json::to_vec(&value)?.len() > crate::conversation::MAX_WIRE_BYTES {
        return Err("schedule response exceeds24MiB".into());
    }
    Ok(value)
}
