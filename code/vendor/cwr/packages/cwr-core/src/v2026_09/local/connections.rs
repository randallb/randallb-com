//! Explicit input selection, shared by setup, graph loading, and execution.
use super::*;
use crate::credentials::LegacyAccess;
use crate::workspace_environment::{EnvironmentAccess, EnvironmentEntryKind};
use crate::{
    input_delivery::{InputContext, InputResolution, InputSelection},
    v2026_09::graph::{
        self,
        requirements::{InputDeclaration, PlaintextStorage, PLAINTEXT_TYPE, SECRET_TYPE},
    },
};
mod definition;
mod handoff;
mod mutation;
mod resolution;
mod storage;
mod target;
use definition::*;
pub(crate) use handoff::{prepare_handoff, InputHandoff};
pub use mutation::{save_plaintext_connection, save_project_connections};
pub(crate) use resolution::capture_for_binding;
use resolution::*;
use storage::*;
pub(super) use target::configured_target;
use target::*;
#[cfg(test)]
mod handoff_tests;
#[cfg(test)]
#[path = "connections_tests.rs"]
mod tests;
const PREFIX: &str = "cwr-local-key:";

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConnectionRequest {
    Available {
        path: PathBuf,
    },
    Inspect {
        path: PathBuf,
    },
    Save {
        path: PathBuf,
        fingerprint: String,
        grants_revision: String,
        assignments: BTreeMap<String, String>,
    },
    SavePlaintext {
        path: PathBuf,
        fingerprint: String,
        grants_revision: String,
        alias: String,
        value: Value,
    },
}

#[derive(Serialize)]
pub struct ProjectConnections {
    pub fingerprint: String,
    pub grants_revision: String,
    pub requirements: Vec<ProjectConnection>,
    pub environment_entries: Vec<crate::workspace_environment::EnvironmentEntry>,
    pub environment_unavailable: bool,
    pub warnings: Vec<graph::GraphLoadWarning>,
}

#[derive(Serialize)]
pub struct ProjectConnection {
    pub alias: String,
    pub type_ref: String,
    pub purpose: String,
    pub required: bool,
    pub key_id: Option<String>,
    pub approved: bool,
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_value: Option<Value>,
    pub plaintext_storage: Option<PlaintextStorage>,
    pub destinations: Vec<String>,
}

pub fn project_connections_available(path: &Path) -> CwrResult<bool> {
    if path
        .file_name()
        .is_some_and(|name| name == "WORKSPACE.toml")
    {
        return Ok(LocalWorkspaceComposition::load(path)?.runtimes.len() == 1);
    }
    canonical_cwr_path(path)?;
    Ok(true)
}

pub fn inspect_project_connections(path: &Path) -> CwrResult<ProjectConnections> {
    let target = connection_target(path)?;
    let definition = definition_for_target(&target)?;
    let grants = read_grants(&target.grants_path)?;
    let resolved = resolve(&target, &definition, &grants, None)?;
    Ok(report(&target, &definition, &grants, resolved.rows))
}

pub(super) fn graph_options(path: &Path) -> CwrResult<graph::GraphLoadOptions> {
    let target = connection_target(path)?;
    if uses_managed_setup(&target)? {
        options_for_target(&target)
    } else {
        Ok(target.options)
    }
}

pub(super) fn options_for_target(target: &ConnectionTarget) -> CwrResult<graph::GraphLoadOptions> {
    let definition = definition_for_target(target)?;
    let grants = read_grants(&target.grants_path)?;
    let resolved = resolve(target, &definition, &grants, Some(&BTreeSet::new()))?;
    if resolved.rows.iter().any(|r| {
        r.required && !r.configured && r.plaintext_storage == Some(PlaintextStorage::LocalJson)
    }) {
        return invalid("A required local configuration input is missing.");
    }
    Ok(resolved.options)
}

pub(super) fn resolve_local_graph(path: &Path) -> CwrResult<ResolvedCwrGraph> {
    graph::resolve_graph_with_options(path, &graph::EmptyResourceResolver, &graph_options(path)?)
}

// Validation/materialization is performed at the actual process handoff.
pub(super) fn deliver(
    _path: &Path,
    route: &mut crate::adapter::ResolvedProcessAdapterRoute,
    invocation: &mut AdapterInvocation,
) -> CwrResult<()> {
    handoff::retained_inputs(route, invocation).map(|_| ())
}

pub(super) fn uses_managed_setup(target: &ConnectionTarget) -> CwrResult<bool> {
    let declarations = graph::requirements::inspect_input_declarations(&target.path)?;
    if declarations.is_empty() {
        return Ok(false);
    }
    let grants = read_grants(&target.grants_path)?;
    Ok(!grants.connections.is_empty()
        || !grants.plaintext.is_empty()
        || declarations
            .iter()
            .any(|d| d.storage == Some(PlaintextStorage::WorkspaceVariable)))
}
