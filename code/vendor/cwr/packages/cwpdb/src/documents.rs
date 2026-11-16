//! Declared revision-checked, graph-backed domain document operations.
//! Domain authorization and schema validation are supplied by trusted owners.
use crate::{
    invocation::{Authorizer, Context},
    CwpdbResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
mod sqlite;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub reference: String,
    pub schema_ref: String,
    pub value: Map<String, Value>,
    pub revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Create {
        schema_ref: String,
        value: Map<String, Value>,
    },
    Replace {
        value: Map<String, Value>,
    },
    Patch {
        patch: Map<String, Value>,
    },
    Delete,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
    pub context: Context,
    pub reference: String,
    pub request_id: String,
    pub expected_revision: Option<String>,
    pub operation: Operation,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    Committed {
        record: Option<Record>,
        #[serde(skip_serializing_if = "Option::is_none")]
        deleted_revision: Option<String>,
    },
    CommittedResultExpired,
}
/// Domain validators check complete values and immutable facts. They receive no
/// native handle; failure rolls back record, revision and receipt together.
pub trait Validator {
    fn validate(
        &self,
        schema_ref: &str,
        previous: Option<&Map<String, Value>>,
        next: &Map<String, Value>,
    ) -> CwpdbResult<()>;
}
pub trait DocumentStorage {
    fn reconcile_document(
        &mut self,
        request: &Mutation,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<Outcome>>;
    fn get_document(
        &mut self,
        context: &Context,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<Record>>;
    fn mutate_document(
        &mut self,
        request: &Mutation,
        authorizer: &dyn Authorizer,
        validator: &dyn Validator,
    ) -> CwpdbResult<Outcome>;
}
