//! Payload-free domain errors; cwpdb owns physical backend classification.
use super::{OperationError, OperationErrorCode};
pub fn map(error: Box<dyn std::error::Error + Send + Sync>) -> OperationError {
    if error.is::<crate::entities::sidebar_pins::RevisionConflict>() {
        OperationError {
            code: OperationErrorCode::RevisionConflict,
        }
    } else {
        crate::cwpdb::desktop::domain_errors::map(error)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pin_conflict_keeps_domain_code() {
        assert_eq!(
            map(crate::entities::sidebar_pins::RevisionConflict.into()).code,
            OperationErrorCode::RevisionConflict
        );
    }
}
