//! Shared runtime, resource, action, conversation, and adapter boundaries for CWR.

extern crate self as cwr_core;

pub mod action_contract;
pub mod adapter;
pub mod cloud_execution;
mod configuration;
pub mod credentials;
/// CWR namespace for the shared cwpDb crate.
pub mod cwpdb {
    #[cfg(test)]
    pub use cwpdb_store::test_support;
    // Only storage-owned domain operations and opaque constructors cross CWR.
    // SQL text, physical IDs/connections and transaction callbacks stay in CWPDB.
    pub use cwpdb_store::{
        conversation, declared, derived_ref, desktop, documents, environment, invocation, metrics,
        posthog, profiling, remote, schedules, schema_ref, CwpdbResult, FfiStore, IdentityClaim,
        NativeStore, EDGE_SCHEMA_REF,
    };
    pub mod ref_read {
        pub use cwpdb_store::ref_read::{RefReadError, RefRecord, RefRevision, RefValue};
    }
}
pub mod desktop_storage;
mod diagnostics;
pub mod entities;
mod error;
pub mod input_delivery;
pub mod intrinsic_cwr;
mod journal;
mod local_lock;
mod metrics;
pub mod open_responses;
pub mod refs;
pub mod resources;
pub mod token_estimation;
pub mod tool_names;
pub mod v2026_09;
pub mod workspace_environment;
pub mod workspace_execution;

pub use diagnostics::{capture_resident_stderr, ResidentDiagnostics};
pub use error::{ConfigurationError, CwrResult};
pub use metrics::{
    flush_posthog_metrics, posthog_environment_fingerprint, LOCAL_METRICS_DATABASE_FILENAME,
};
pub use refs::{ActionSelector, ActionSelectorError, CoworkerRef, CwrRef, CwrRefError, CwrRoot};
pub use resources::{
    materialize_resource, put_directory_resource, ResourceReference, RunResourceStore,
};
pub use tool_names::{project_tool_name, ToolNameError};
