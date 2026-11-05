//! Disposable executor transport for existing CWR process-adapter invocations.
//!
//! CWR admission, authority, child invocation linkage, and result validation stay
//! with the coordinator. This module never admits work on an executor's behalf.

mod transport;
pub use transport::{run_proxy, CloudAssignment, CloudDispatch, CloudEvent, CloudEvents};

mod coordinator;
pub use coordinator::{CloudCallback, CloudCoordinator, CloudInterruption};

mod recovery;

mod service;
pub use service::{
    CloudExecutorEndpoint, CloudHttpResponse, CloudHttpService, CloudModelGrant, CloudServiceConfig,
};

mod service_store;
pub use service_store::CloudStorage;

mod storage_transport;
pub use storage_transport::HttpDeclaredTransport;
