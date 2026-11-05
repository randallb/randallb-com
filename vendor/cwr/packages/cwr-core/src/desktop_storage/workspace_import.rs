//! Named storage operation; declaration validation remains owned by CWR.
use super::{OperationError, OperationErrorCode};
pub use crate::cwpdb::desktop::workspace_import::Response;
use crate::cwpdb::NativeStore;
pub fn execute(store: &mut NativeStore, source: &str) -> Result<Response, OperationError> {
    let declarations = super::declarations::catalog().map_err(|_| OperationError {
        code: OperationErrorCode::StorageFailure,
    })?;
    crate::cwpdb::desktop::workspace_import::execute(store, declarations, source)
}
