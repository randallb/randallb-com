//! One finite Desktop operation on the storage owner's existing handle.
use super::{history, ordering, personal_collections, registry, versions, workspace_import};
use crate::{declared::Catalog, NativeStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
const VARIABLE: &str = "cwp://org.coworkerprotocol/cwr/entities/variable";
#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopOperation {
    Ordering {
        request: ordering::Request,
    },
    PersonalCollections {
        request: personal_collections::Request,
    },
    History {
        request: history::Request,
    },
    WorkspaceImport {
        source: String,
    },
    Versions {
        request: versions::Request,
    },
    Registry {
        request: registry::Request,
    },
    GetVariable {
        reference: String,
    },
    CreateVariable {
        reference: String,
        value: Value,
    },
    ReplaceVariable {
        reference: String,
        expected_revision: String,
        value: Value,
    },
}

impl DesktopOperation {
    pub fn is_write(&self) -> bool {
        match self {
            Self::Ordering { request } => ordering::is_write(request),
            Self::PersonalCollections { request } => personal_collections::is_write(request),
            Self::History { request } => history::is_write(request),
            Self::Registry { request } => request.is_write(),
            Self::Versions { request } => request.is_write(),
            Self::WorkspaceImport { .. } => true,
            Self::GetVariable { .. } => false,
            _ => true,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DesktopRecord {
    pub reference: String,
    pub value: Value,
    /// Opaque string: JSON numbers cannot preserve all SQLite revisions.
    pub revision: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DesktopResult {
    Ordering {
        result: ordering::Response,
    },
    PersonalCollections {
        result: personal_collections::Response,
    },
    History {
        result: history::Response,
    },
    WorkspaceImport {
        result: workspace_import::Response,
    },
    Versions {
        result: versions::Response,
    },
    Registry {
        result: registry::Response,
    },
    Committed {
        record: DesktopRecord,
    },
    Rejected {
        code: &'static str,
    },
}

pub fn execute(
    store: &mut NativeStore,
    catalog: &Catalog,
    operation: DesktopOperation,
) -> DesktopResult {
    if let DesktopOperation::Ordering { request } = operation {
        return match ordering::execute(store, catalog, request) {
            Ok(result) => DesktopResult::Ordering { result },
            Err(error) => DesktopResult::Rejected {
                code: error.code.as_str(),
            },
        };
    }
    if let DesktopOperation::PersonalCollections { request } = operation {
        return match personal_collections::execute(store, catalog, request) {
            Ok(result) => DesktopResult::PersonalCollections { result },
            Err(error) => DesktopResult::Rejected {
                code: error.code.as_str(),
            },
        };
    }
    if let DesktopOperation::History { request } = operation {
        return match history::execute(store, catalog, request) {
            Ok(result) => DesktopResult::History { result },
            Err(error) => DesktopResult::Rejected {
                code: error.code.as_str(),
            },
        };
    }
    if let DesktopOperation::WorkspaceImport { source } = operation {
        return match workspace_import::execute(store, catalog, &source) {
            Ok(result) => DesktopResult::WorkspaceImport { result },
            Err(error) => DesktopResult::Rejected {
                code: error.code.as_str(),
            },
        };
    }
    if let DesktopOperation::Versions { request } = operation {
        return match versions::execute(store, catalog, request) {
            Ok(result) => DesktopResult::Versions { result },
            Err(error) => DesktopResult::Rejected {
                code: error.code.as_str(),
            },
        };
    }
    if let DesktopOperation::Registry { request } = operation {
        return match registry::execute(store, catalog, request) {
            Ok(result) => DesktopResult::Registry { result },
            Err(error) => DesktopResult::Rejected {
                code: error.code.as_str(),
            },
        };
    }
    use super::variable::{self, Request};
    let (reference, read) = match &operation {
        DesktopOperation::GetVariable { reference } => (reference, true),
        DesktopOperation::CreateVariable { reference, .. }
        | DesktopOperation::ReplaceVariable { reference, .. } => (reference, false),
        _ => unreachable!("domain dispatched above"),
    };
    let valid_reference = catalog.validate_reference(reference);
    let result = (|| {
        valid_reference?;
        let request = match operation {
            DesktopOperation::GetVariable { reference } => Request::Read { reference },
            DesktopOperation::CreateVariable { reference, value } => {
                Request::Create { reference, value }
            }
            DesktopOperation::ReplaceVariable {
                reference,
                expected_revision,
                value,
            } => Request::Replace {
                reference,
                expected_revision,
                value,
            },
            _ => unreachable!("domain dispatched above"),
        };
        let record = variable::execute(store, &catalog.entity(VARIABLE)?, request)?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(DesktopRecord {
            reference: record.reference,
            value: record.value.properties,
            revision: record.revision.legacy_counter().to_string(),
        })
    })();
    match result {
        Ok(record) => DesktopResult::Committed { record },
        Err(error) => DesktopResult::Rejected {
            // Do not forward raw SQLite messages or protected values.
            code: if read {
                "unavailable"
            } else {
                match error.to_string().as_str() {
                    "entity revision conflict" => "revision_conflict",
                    "entity value does not satisfy its definition" => "invalid_value",
                    "resource not found" => "not_found",
                    _ => "storage_failure",
                }
            },
        },
    }
}
