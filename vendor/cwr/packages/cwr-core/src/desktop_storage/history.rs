//! Named history operations; cwpdb owns transactions and physical retention.
use super::{OperationError, OperationErrorCode};
pub use crate::cwpdb::desktop::history::{is_write, Request, Response};
use crate::cwpdb::NativeStore;
pub fn execute(store: &mut NativeStore, request: Request) -> Result<Response, OperationError> {
    let declarations = super::declarations::catalog().map_err(|_| OperationError {
        code: OperationErrorCode::StorageFailure,
    })?;
    crate::cwpdb::desktop::history::execute(store, declarations, request)
}
