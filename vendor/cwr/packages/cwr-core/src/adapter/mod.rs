//! Provider-neutral process transport, protocol, resources, and route resolution.

mod decisions_endpoint;
pub use decisions_endpoint::{decisions_endpoint, decisions_endpoint_is_loopback};
mod codex_login_repair;
pub use codex_login_repair::run_codex_login_repair;
mod import;
mod storage;
pub use import::import_adapter_home;
use storage::shared_codex_home;
pub use storage::{
    adapter_data_directory, adapter_data_directory_path, configure_adapter,
    prepare_adapter_data_directory, AdapterConfiguration,
};
mod repair;
pub use repair::{adapter_setup_readiness, run_adapter_repair};
mod input_slots;
mod resource;
pub(crate) use input_slots::omit_input;
pub mod user_input;
pub const PERSON_ADAPTER_REF: &str = "cwp://org.coworkerprotocol/2026-09/adapters/person";

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::{OsStr, OsString},
    fmt,
    io::{self, BufRead, BufReader, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::PermissionsExt,
            net::UnixStream,
            process::{CommandExt, ExitStatusExt},
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant},
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

use crate::CwrResult;
use cwr_core::CwrRef;

pub(crate) use resource::{
    built_in_adapter_resource, is_builtin_adapter_resource, BuiltInAdapterResource,
};

include!("model.rs");
include!("activity.rs");
include!("shutdown.rs");
include!("timing.rs");
include!("validation.rs");
include!("process.rs");
include!("protocol.rs");
include!("readiness.rs");
include!("codex_model_catalog.rs");
include!("route.rs");
include!("claude_route.rs");

include!("claude_readiness.rs");

#[cfg(test)]
mod readiness_tests;
