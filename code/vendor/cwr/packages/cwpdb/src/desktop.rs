//! Complete native Desktop storage operations. Declarations remain owned by CWR.
use serde::Serialize;
pub mod domain_errors;
pub mod history;
pub mod operation;
pub mod ordering;
pub mod personal_collections;
mod receipts;
pub mod registry;
pub mod variable;
pub mod versions;
pub mod workspace_import;
/// Code-only errors for operation modules; never serialize backend messages.
#[derive(Debug, Serialize)]
pub struct OperationError {
    pub code: OperationErrorCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationErrorCode {
    InvalidInput,
    InvalidValue,
    Unavailable,
    NotFound,
    AlreadyExists,
    RefRetired,
    RevisionConflict,
    EdgeConflict,
    RelationshipsExist,
    RequestConflict,
    UnsupportedDomainContract,
    InvalidContinuation,
    ReadOnly,
    ServiceLimit,
    DatabaseFull,
    StorageFailure,
}

impl OperationErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::InvalidValue => "invalid_value",
            Self::Unavailable => "unavailable",
            Self::NotFound => "not_found",
            Self::AlreadyExists => "already_exists",
            Self::RefRetired => "ref_retired",
            Self::RevisionConflict => "revision_conflict",
            Self::EdgeConflict => "edge_conflict",
            Self::RelationshipsExist => "relationships_exist",
            Self::RequestConflict => "request_conflict",
            Self::UnsupportedDomainContract => "unsupported_domain_contract",
            Self::InvalidContinuation => "invalid_continuation",
            Self::ReadOnly => "read_only",
            Self::ServiceLimit => "service_limit",
            Self::DatabaseFull => "database_full",
            Self::StorageFailure => "storage_failure",
        }
    }
}

impl std::fmt::Display for OperationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Codes contain no domain payload or backend diagnostics.
        write!(formatter, "{:?}", self.code)
    }
}

impl std::error::Error for OperationError {}

#[cfg(test)]
mod test_catalog;
#[cfg(test)]
pub(crate) use test_catalog::{catalog as test_catalog, entity_fixture};
