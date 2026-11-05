//! Host-retained input selections. These records contain identity, never managed values.
use crate::v2026_09::graph::requirements::InputSource;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputDestination {
    ShellEnv { name: String },
    OpenResponsesApiKey,
}

impl InputDestination {
    pub(crate) fn pointer(&self) -> String {
        match self {
            Self::ShellEnv { name } => {
                format!("/env/{}", name.replace('~', "~0").replace('/', "~1"))
            }
            Self::OpenResponsesApiKey => "/api_key".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InputUse {
    pub source: InputSource,
    pub destination: InputDestination,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputSelection {
    Workspace {
        reference: String,
        revision: i64,
        entry_kind: crate::workspace_environment::EnvironmentEntryKind,
        generation: String,
    },
    Legacy {
        key_id: String,
        generation: String,
    },
    Absent,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InputDependency {
    pub source: InputSource,
    pub selection: InputSelection,
    pub destinations: Vec<InputDestination>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InputContext {
    pub path: PathBuf,
    pub workspace: PathBuf,
    pub grants_path: PathBuf,
    pub runtime_ref: Option<String>,
    pub storage_identity: String,
    pub definition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InvocationInputs {
    pub version: u8,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub binding_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<InputContext>,
    pub dependencies: Vec<InputDependency>,
}

impl Default for InvocationInputs {
    fn default() -> Self {
        Self {
            version: 1,
            binding_ref: String::new(),
            context: None,
            dependencies: Vec::new(),
        }
    }
}

/// A graph-load snapshot. Projection selects only the chosen binding's inputs.
#[derive(Debug, Clone, Default)]
pub struct InputResolution {
    pub enforce_destinations: bool,
    pub context: Option<InputContext>,
    pub selections: BTreeMap<String, InputSelection>,
}

impl InputResolution {
    pub(crate) fn project(&self, binding: &str, uses: &[InputUse]) -> InvocationInputs {
        let mut dependencies: Vec<InputDependency> = Vec::new();
        for usage in uses {
            // A source outside the selected root cannot acquire authority by
            // colliding with one of its aliases. Direct graph callers lack approval.
            let unavailable = InputSelection::Unavailable;
            let selection = if self
                .context
                .as_ref()
                .is_some_and(|c| c.path.to_string_lossy() == usage.source.scope)
            {
                let Some(selection) = self.selections.get(&usage.source.alias) else {
                    continue;
                };
                selection
            } else {
                &unavailable
            };
            if let Some(dependency) = dependencies.iter_mut().find(|d| d.source == usage.source) {
                dependency.destinations.push(usage.destination.clone());
            } else {
                dependencies.push(InputDependency {
                    source: usage.source.clone(),
                    selection: selection.clone(),
                    destinations: vec![usage.destination.clone()],
                });
            }
        }
        if uses.is_empty() {
            InvocationInputs::default()
        } else {
            InvocationInputs {
                version: 1,
                binding_ref: binding.into(),
                context: self.context.clone(),
                dependencies,
            }
        }
    }
}

impl InvocationInputs {
    /// Called only when projecting new work. Restore and retry deserialize the
    /// retained record and never refresh it from current assignments.
    pub(crate) fn admit(&self, uses: &[InputUse]) -> Self {
        crate::v2026_09::local::connections::capture_for_binding(self, uses).unwrap_or_else(|_| {
            let mut blocked = self.clone();
            blocked.dependencies = uses
                .iter()
                .map(|u| InputDependency {
                    source: u.source.clone(),
                    selection: InputSelection::Unavailable,
                    destinations: vec![u.destination.clone()],
                })
                .collect();
            blocked
        })
    }
}
