// Compiled through the integration-test harness so Cargo supplies the adapter
// binary path while the adapter-specific cases stay beside the implementation.
use cwr::adapter::{
    run_process_adapter, run_process_adapter_with_activity_and_host_calls,
    run_process_adapter_with_controlled_host_calls, run_process_adapter_with_host_calls,
    run_process_adapter_with_model_output_activity_and_controlled_host_calls, AdapterActivityEvent,
    AdapterHostCallOutcome, AdapterHostOperation, AdapterInvocation, AdapterModelOutputMailbox,
    ProcessAdapterFailure, ProcessAdapterOutcome, ProcessAdapterStopControl,
    ResolvedProcessAdapter, CODEX_ADAPTER_REF,
};
use cwr::CwrRef;
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
include!("tests/fake_codex.rs");
include!("tests/support.rs");
include!("tests/lifecycle.rs");
include!("tests/failures.rs");
include!("tests/context.rs");
include!("tests/commentary.rs");
include!("tests/host_calls.rs");
include!("tests/stop.rs");
include!("tests/workspace.rs");
include!("tests/timing.rs");

include!("tests/working.rs");

include!("tests/user_input.rs");

include!("tests/service_tier.rs");

include!("tests/files.rs");
