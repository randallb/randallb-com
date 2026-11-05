//! Payload-free storage failure classification.
use super::{OperationError, OperationErrorCode};
pub fn map(error: Box<dyn std::error::Error + Send + Sync>) -> OperationError {
    let code = if error.is::<crate::sidebar_pins::RevisionConflict>() {
        OperationErrorCode::RevisionConflict
    } else if error.is::<crate::sequence_receipt::RequestConflict>() {
        OperationErrorCode::RequestConflict
    } else if matches!(error.downcast_ref::<rusqlite::Error>(),Some(rusqlite::Error::SqliteFailure(e,_)) if e.code==rusqlite::ErrorCode::DiskFull)
    {
        OperationErrorCode::DatabaseFull
    } else {
        OperationErrorCode::StorageFailure
    };
    OperationError { code }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_failures_return_only_safe_codes() {
        assert_eq!(
            map(crate::sequence_receipt::RequestConflict.into()).code,
            OperationErrorCode::RequestConflict
        );
        let error = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
            Some("protected backend detail".into()),
        );
        let result = map(error.into());
        assert_eq!(result.code, OperationErrorCode::DatabaseFull);
        assert_eq!(
            serde_json::to_string(&result).unwrap(),
            "{\"code\":\"database_full\"}"
        );
    }
}
