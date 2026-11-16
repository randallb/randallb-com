//! CWR supplies declarations; cwpdb owns every finite operation and its handle.
use crate::cwpdb::{declared::Catalog, FfiStore, NativeStore};
mod declarations;
pub mod domain_errors;
pub mod history;
pub mod ordering;
pub mod personal_collections;
pub mod registry;
pub mod versions;
pub mod workspace_import;
pub use crate::cwpdb::desktop::operation::{DesktopOperation, DesktopRecord, DesktopResult};
pub use crate::cwpdb::desktop::{OperationError, OperationErrorCode};
pub struct DesktopStorage {
    catalog: &'static Catalog,
}
impl DesktopStorage {
    pub fn new() -> crate::CwrResult<Self> {
        Ok(Self {
            catalog: declarations::catalog()?,
        })
    }
    pub fn execute(&self, store: &mut NativeStore, operation: DesktopOperation) -> DesktopResult {
        crate::cwpdb::desktop::operation::execute(store, self.catalog, operation)
    }
    pub fn execute_ffi(&self, handle: &mut FfiStore, operation: DesktopOperation) -> DesktopResult {
        handle.desktop_operation(self.catalog, operation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_revisions_above_javascript_integer_precision() -> crate::CwrResult<()> {
        let facade = DesktopStorage::new()?;
        let mut store = NativeStore::open_in_memory()?;
        let reference = "cwp://test.local/variables/large-revision";
        let value = json!({"name": "LARGE", "value": "before"});
        assert!(matches!(
            facade.execute(
                &mut store,
                DesktopOperation::CreateVariable {
                    reference: reference.into(),
                    value: value.clone(),
                }
            ),
            DesktopResult::Committed { .. }
        ));
        crate::cwpdb::test_support::entity_revision(&mut store, reference, 9007199254740993)?;
        let result = facade.execute(
            &mut store,
            DesktopOperation::GetVariable {
                reference: reference.into(),
            },
        );
        let DesktopResult::Committed { record } = result else {
            panic!("expected committed read")
        };
        assert_eq!(record.revision, "9007199254740993");
        let updated = facade.execute(
            &mut store,
            DesktopOperation::ReplaceVariable {
                reference: reference.into(),
                expected_revision: record.revision,
                value: json!({"name": "LARGE", "value": "after"}),
            },
        );
        let DesktopResult::Committed { record } = updated else {
            panic!("expected committed replacement")
        };
        assert_eq!(record.revision, "9007199254740994");
        Ok(())
    }

    #[test]
    fn cached_variable_schema_reads_mutations_from_another_handle() -> crate::CwrResult<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("variables.db");
        let first = DesktopStorage::new()?;
        let second = DesktopStorage::new()?;
        assert!(std::ptr::eq(first.catalog, second.catalog));
        let mut first_store = NativeStore::open(&path)?;
        let mut second_store = NativeStore::open(&path)?;
        let reference = "cwp://test.local/variables/cross-handle";
        let created = first.execute(
            &mut first_store,
            DesktopOperation::CreateVariable {
                reference: reference.into(),
                value: json!({"name": "SHARED", "value": "before"}),
            },
        );
        assert!(matches!(created, DesktopResult::Committed { .. }));
        let DesktopResult::Committed { record } = second.execute(
            &mut second_store,
            DesktopOperation::GetVariable {
                reference: reference.into(),
            },
        ) else {
            panic!("second handle did not read the first handle's write")
        };
        let replaced = second.execute(
            &mut second_store,
            DesktopOperation::ReplaceVariable {
                reference: reference.into(),
                expected_revision: record.revision,
                value: json!({"name": "SHARED", "value": "after"}),
            },
        );
        assert!(matches!(replaced, DesktopResult::Committed { .. }));
        let DesktopResult::Committed { record } = first.execute(
            &mut first_store,
            DesktopOperation::GetVariable {
                reference: reference.into(),
            },
        ) else {
            panic!("first handle did not read the second handle's write")
        };
        assert_eq!(record.value["value"], "after");
        Ok(())
    }
    #[test]
    fn history_and_pin_reads_use_readonly_snapshot() -> crate::CwrResult<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("read");
        let mut store = NativeStore::open(&path)?;
        history::execute(&mut store, history::Request::Initialize)?;
        drop(store);
        let mut store = NativeStore::open_read_only(&path)?;
        assert_eq!(
            history::execute(&mut store, history::Request::Account)?.result["rows"],
            0
        );
        ordering::execute(
            &mut store,
            ordering::Request::Read {
                scope: crate::entities::sidebar_pins::PinScope {
                    workspace: "/fixture".into(),
                    viewer: "cwp://example.test/viewer".into(),
                },
            },
        )?;
        Ok(())
    }
}
