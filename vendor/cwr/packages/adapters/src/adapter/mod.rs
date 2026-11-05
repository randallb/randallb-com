//! Adapter-side implementations of the CWR process-adapter protocol.

use std::{
    io::{self, BufRead, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::CwrResult;
pub use cwr::adapter::*;

mod activated;
pub mod claude;
pub mod codex;
mod context;
pub mod decisions;
pub mod open_responses;
mod tools;

include!("loopback.rs");
include!("rng.rs");
include!("sh.rs");

fn default_true() -> bool {
    true
}

fn provider_store_marker() -> CwrResult<Option<&'static str>> {
    match std::env::var_os("CWR_PROVIDER_STORE") {
        None => Ok(None),
        Some(value) if value.as_os_str().to_str() == Some("managed") => Ok(Some("managed")),
        Some(value) if value.as_os_str().to_str() == Some("shared") => Ok(Some("shared")),
        Some(value) if value.as_os_str().to_str() == Some("system") => Ok(Some("system")),
        Some(_) => {
            protocol_failure("CWR_PROVIDER_STORE must be managed, shared, or system when set")
        }
    }
}

fn provider_store_matches(state: &Value, expected: Option<&str>) -> bool {
    match (state.get("provider_store"), expected) {
        (None, None) => true,
        (Some(Value::String(actual)), Some(expected)) => actual == expected,
        _ => false,
    }
}

fn protocol_failure<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(protocol_error(message))
}

fn protocol_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message.into()).into()
}
