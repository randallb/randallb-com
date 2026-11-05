//! Named Desktop storage operations; cwpdb owns atomic persistence.
use super::{OperationError, OperationErrorCode};
pub use crate::cwpdb::desktop::ordering::{is_write, Request, Response};
use crate::cwpdb::NativeStore;
pub fn execute(store: &mut NativeStore, request: Request) -> Result<Response, OperationError> {
    let declarations = super::declarations::catalog().map_err(|_| OperationError {
        code: OperationErrorCode::StorageFailure,
    })?;
    crate::cwpdb::desktop::ordering::execute(store, declarations, request)
}
