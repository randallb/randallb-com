//! Named storage operation; declaration validation remains owned by CWR.
use super::{OperationError, OperationErrorCode};
pub use crate::cwpdb::desktop::versions::{
    File, Instance, Request, Response, SelectionPolicy, Version,
};
use crate::cwpdb::NativeStore;
pub fn execute(store: &mut NativeStore, request: Request) -> Result<Response, OperationError> {
    let declarations = super::declarations::catalog().map_err(|_| OperationError {
        code: OperationErrorCode::StorageFailure,
    })?;
    crate::cwpdb::desktop::versions::execute(store, declarations, request)
}
