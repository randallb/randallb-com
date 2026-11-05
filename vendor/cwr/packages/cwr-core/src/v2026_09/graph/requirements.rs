use super::*;

pub const PLAINTEXT_TYPE: &str = "cwp://org.coworkerprotocol/2026-09/schemas/plaintext";
pub const SECRET_TYPE: &str = "cwp://org.coworkerprotocol/2026-09/schemas/secret";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PlaintextStorage {
    LocalJson,
    WorkspaceVariable,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDeclaration {
    #[serde(rename = "type")]
    pub type_ref: String,
    #[serde(rename = "as")]
    pub alias: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default = "required_by_default")]
    pub required: bool,
    /// Plaintext inputs keep their established local JSON editor unless the
    /// declaration explicitly opts into a workspace variable.
    #[serde(default)]
    pub storage: Option<PlaintextStorage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeDeclaration {
    #[serde(rename = "as")]
    pub alias: String,
    pub access: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default = "required_by_default")]
    pub required: bool,
}

pub(crate) fn required_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequirementStatus {
    Supplied,
    Missing,
    Invalid,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct InputSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<PlaintextStorage>,
    pub scope: String,
    pub alias: String,
    pub type_ref: String,
}

/// Contains names and availability only, never input values or secret handles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Requirement {
    pub scope: String,
    pub alias: String,
    pub type_ref: String,
    pub purpose: String,
    pub required: bool,
    pub status: RequirementStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<InputSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "loop_name")]
    pub runtime_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_access: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_access: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementIssue {
    pub scope: String,
    pub status: RequirementStatus,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputInspection {
    pub requirements: Vec<Requirement>,
    pub issues: Vec<RequirementIssue>,
}

// Missing inputs are Rust values, never special objects inserted into user JSON.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InputBinding {
    pub value: Option<Value>,
    pub sources: Vec<InputSource>,
    pub status: RequirementStatus,
    pub optional: bool,
    pub environment: Option<String>,
    pub unresolved: Option<BindingExpression>,
}

// Preserve the shape of unresolved forwarding internally, including literal siblings.
// Equal provenance alone does not imply equal eventual objects or arrays.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BindingExpression {
    Value(Value),
    Bound(Value, Vec<InputSource>),
    Source(Vec<InputSource>, Option<String>),
    Object(BTreeMap<String, BindingExpression>),
    Array(Vec<BindingExpression>),
}

impl BindingExpression {
    pub(super) fn is_known_string(&self) -> bool {
        matches!(self, Self::Source(origins, environment)
            if environment.is_some() || origins.iter().any(|origin| origin.type_ref == SECRET_TYPE || origin.storage == Some(PlaintextStorage::WorkspaceVariable)))
    }

    fn inspection_value(
        &self,
        path: &str,
        unresolved: &mut BTreeMap<String, Self>,
        sources: &mut BTreeMap<String, Vec<InputSource>>,
    ) -> Value {
        match self {
            Self::Value(value) => value.clone(),
            Self::Bound(value, origins) => {
                sources.insert(path.into(), origins.clone());
                value.clone()
            }
            Self::Source(origins, _) => {
                sources.insert(path.into(), origins.clone());
                unresolved.insert(path.into(), self.clone());
                // Preserve a guaranteed JSON kind without reading the value.
                if self.is_known_string() {
                    Value::String(String::new())
                } else {
                    Value::Null
                }
            }
            Self::Object(entries) => Value::Object(
                entries
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.clone(),
                            value.inspection_value(
                                &parameters::child_pointer(path, name),
                                unresolved,
                                sources,
                            ),
                        )
                    })
                    .collect(),
            ),
            Self::Array(entries) => Value::Array(
                entries
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        value.inspection_value(
                            &parameters::child_pointer(path, &index.to_string()),
                            unresolved,
                            sources,
                        )
                    })
                    .collect(),
            ),
        }
    }
}

pub(crate) type InputBindings = BTreeMap<String, InputBinding>;

impl InputBinding {
    pub(crate) fn omitted(&self) -> bool {
        self.optional && self.value.is_none() && self.status == RequirementStatus::Missing
    }

    fn expression(&self) -> BindingExpression {
        if let Some(expression) = &self.unresolved {
            return expression.clone();
        }
        match &self.value {
            Some(value) if !self.sources.is_empty() => {
                BindingExpression::Bound(value.clone(), self.sources.clone())
            }
            Some(value) => BindingExpression::Value(value.clone()),
            None => self.unresolved.clone().unwrap_or_else(|| {
                BindingExpression::Source(self.sources.clone(), self.environment.clone())
            }),
        }
    }

    pub(in crate::v2026_09::graph) fn inspection_value(&self) -> Parameters {
        let mut unresolved = BTreeMap::new();
        let mut sources = BTreeMap::new();
        let value = self
            .expression()
            .inspection_value("", &mut unresolved, &mut sources);
        Parameters {
            value,
            unresolved,
            sources,
        }
    }

    pub(crate) fn concrete(value: Value) -> Self {
        Self {
            value: Some(value),
            sources: Vec::new(),
            status: RequirementStatus::Supplied,
            optional: false,
            environment: None,
            unresolved: None,
        }
    }
    pub(crate) fn missing() -> Self {
        Self {
            value: None,
            sources: Vec::new(),
            status: RequirementStatus::Missing,
            optional: false,
            environment: None,
            unresolved: None,
        }
    }
}

pub(crate) fn concrete_bindings(value: &Value) -> CwrResult<InputBindings> {
    value
        .as_object()
        .ok_or_else(|| invalid_error("inputs must be an object"))?
        .iter()
        .map(|(name, value)| Ok((name.clone(), InputBinding::concrete(value.clone()))))
        .collect()
}

pub(crate) fn binding_values(inputs: &InputBindings) -> Value {
    Value::Object(
        inputs
            .iter()
            .filter_map(|(name, binding)| binding.value.clone().map(|v| (name.clone(), v)))
            .collect(),
    )
}

pub fn inspect_requirements(
    input: impl AsRef<Path>,
    resolver: &dyn CheckResourceResolver,
) -> CwrResult<InputInspection> {
    let path = absolute_path(input.as_ref())?;
    resolver::inspect_manifest_inputs(
        &path,
        resolver,
        &GraphLoadOptions::default(),
        InputBindings::new(),
        true,
    )
}

mod bindings;
pub(crate) use bindings::{bind_template, equivalent_inputs, forward_bindings, InputEquivalence};
mod contract;
pub(super) use contract::check_inputs;

/// Read only the selected declaration's volume contract.
pub fn inspect_volume_declarations(input: impl AsRef<Path>) -> CwrResult<Vec<VolumeDeclaration>> {
    let path = absolute_path(input.as_ref())?;
    let document = read_toml_json(&path)?;
    validate_core_document(&path, document_type(&path, &document)?, &document)?;
    let base: ManifestFile = decode(&path, document)?;
    Ok(base.volumes)
}

pub(crate) fn workspace_input_contract(
    path: &Path,
    declarations: &[InputDeclaration],
    inputs: InputBindings,
) -> CwrResult<(InputBindings, InputInspection)> {
    let base = ManifestFile {
        type_ref: WORKSPACE_TYPE.into(),
        permissions: Default::default(),
        imports: Vec::new(),
        exports: Vec::new(),
        input_schema: None,
        inputs: declarations.to_vec(),
        volumes: Vec::new(),
    };
    let mut report = InputInspection::default();
    let inputs = check_inputs(
        path,
        &base,
        inputs,
        &EmptyResourceResolver,
        Some(&mut report),
        true,
    )?;
    Ok((inputs, report))
}

pub(crate) fn inspect_input_declarations(
    input: impl AsRef<Path>,
) -> CwrResult<Vec<InputDeclaration>> {
    let path = absolute_path(input.as_ref())?;
    let document = read_toml_json(&path)?;
    validate_core_document(&path, document_type(&path, &document)?, &document)?;
    let base: ManifestFile = decode(&path, document)?;
    Ok(base.inputs)
}

#[cfg(test)]
mod tests;

/// Reconfiguration retains the types assigned when host inputs were activated.
pub(crate) fn retained_input_bindings(
    graph: &ResolvedCwrGraph,
    source: &Path,
    values: &Value,
) -> CwrResult<InputBindings> {
    let mut bindings = concrete_bindings(values)?;
    if let Some(declarations) = graph
        .resources
        .iter()
        .find(|resource| resource.source == source)
        .and_then(|resource| resource.document.get("inputs"))
    {
        let declarations: Vec<InputDeclaration> = serde_json::from_value(declarations.clone())?;
        for declaration in declarations {
            if let Some(binding) = bindings.get_mut(&declaration.alias) {
                binding.sources.push(InputSource {
                    scope: source.to_string_lossy().into_owned(),
                    alias: declaration.alias,
                    type_ref: declaration.type_ref,
                    storage: declaration.storage,
                });
            }
        }
    }
    Ok(bindings)
}
