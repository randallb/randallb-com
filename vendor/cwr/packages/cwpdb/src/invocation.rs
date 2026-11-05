//! Declared persistence operations for already-authorized invocation execution.
//!
//! CWR owns action admission and authority. These operations enforce persistence
//! invariants without exposing native transactions or accepting SQL.
use crate::CwpdbResult;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod sqlite;
pub(crate) use sqlite::mutate_in_transaction;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub workspace_ref: String,
    pub caller_ref: String,
}

/// The trusted coordinator implements current authorization. This is called on
/// every operation, including receipt replay and every continuation page.
/// Request context is a lookup input, never proof of authority by itself.
pub trait Authorizer {
    fn authorize(&self, context: &Context, invocation_ref: &str, write: bool) -> CwpdbResult<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub executor_ref: String,
    pub assignment_ref: String,
    pub generation: u64,
    pub route_fingerprint: String,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Progress,
    Completed,
    Interrupted,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub sequence: u64,
    pub kind: EventKind,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Retain normal CWR admission context, not a replacement action engine.
    Admit {
        conversation_ref: String,
        parent_invocation_ref: Option<String>,
        admitted: Value,
    },
    Assign {
        assignment: Assignment,
    },
    Append {
        assignment_ref: String,
        executor_ref: String,
        generation: u64,
        event: Event,
    },
    /// Coordinator interruption/cancellation remains possible after expiry.
    Stop {
        kind: EventKind,
        reason: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
    pub context: Context,
    pub invocation_ref: String,
    pub request_id: String,
    /// Absent only for admission; an opaque token from the preceding read/write.
    pub expected_revision: Option<String>,
    pub operation: Operation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub workspace_ref: String,
    pub invocation_ref: String,
    pub conversation_ref: String,
    pub parent_invocation_ref: Option<String>,
    pub admitted: Value,
    pub assignment: Option<Assignment>,
    pub revision: String,
    pub next_sequence: u64,
    pub terminal: Option<EventKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub invocation_ref: String,
    pub revision: String,
    pub next_sequence: u64,
    pub terminal: Option<EventKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", content = "receipt", rename_all = "snake_case")]
pub enum Outcome {
    Committed(Receipt),
    CommittedResultExpired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EventPage {
    pub items: Vec<Event>,
    /// The last returned sequence; pass it as `after` for the next page.
    pub continuation: Option<u64>,
    pub complete: bool,
}

/// Reconciliation after an acknowledgement is lost. Retained arguments obey the
/// same erasure rules as the original receipt; current authorization is checked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ReceiptLookup {
    NotCommitted,
    Retained {
        request: Box<Mutation>,
        receipt: Receipt,
    },
    CommittedResultExpired,
}

/// Local and hosted implementations must provide identical operation semantics.
/// `now_unix_ms` comes from the trusted coordinator clock, never an executor.
/// Admission + assignment + append are separate atomic logical operations; no
/// transaction handle, physical ID, SQL, or caller-generated index crosses here.
pub trait InvocationStorage {
    fn mutate(
        &mut self,
        request: &Mutation,
        authorizer: &dyn Authorizer,
        now_unix_ms: u64,
    ) -> CwpdbResult<Outcome>;
    fn receipt(
        &mut self,
        context: &Context,
        invocation_ref: &str,
        request_id: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<ReceiptLookup>;
    fn invocation(
        &mut self,
        context: &Context,
        invocation_ref: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<Record>>;
    fn events(
        &mut self,
        context: &Context,
        invocation_ref: &str,
        after: Option<u64>,
        limit: usize,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<EventPage>;
}
