use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::CwrResult;

mod actor;
pub use actor::{CoworkerCategory, ResolvedCwrActor, ResolvedNativeTool};
mod managed;
mod parameters;
mod path;
use parameters::Parameters;
pub mod requirements;
use requirements::{InputDeclaration, VolumeDeclaration};
mod resolver;
mod schema;
use path::{
    absolute_path, pin_regular_file_parent, resolve_regular_file, resolved_regular_file,
    validate_identity_ref,
};
mod state_resources;
use resolver::resolve_manifest_graph;
use schema::{
    is_builtin_adapter, json_schema_validator, load_schema_locator, resolve_adapter,
    validate_adapter_params, validate_core_document, validate_standalone_resource,
};
use state_resources::validate_state_resources;

// Graph declarations share one module; loading and validation are split
// physically so the public graph boundary remains small and inspectable.
include!("model.rs");
include!("loading.rs");
include!("gateway.rs");
include!("validation.rs");
include!("catalog.rs");
include!("conversation_target.rs");
include!("role_context.rs");
include!("helpers.rs");

#[cfg(test)]
#[path = "tests/authority.rs"]
mod authority_tests;

#[cfg(test)]
#[path = "tests/composition.rs"]
mod composition_tests;

#[cfg(test)]
#[path = "tests/file_reads.rs"]
mod file_reads_tests;

mod message_binding;
