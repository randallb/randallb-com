#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VolumeMapping {
    #[serde(rename = "ref")]
    reference: String,
    #[serde(rename = "as")]
    alias: String,
    access: LocalVolumeAccessMode,
}

fn host_bindings(
    values: &Value,
    available_environment: &BTreeSet<String>,
    wrapped: bool,
) -> CwrResult<InputBindings> {
    let mut result = InputBindings::new();
    for (name, value) in values
        .as_object()
        .ok_or_else(|| invalid_error("bindings must be an object"))?
    {
        let env = value
            .as_object()
            .filter(|object| object.len() == 1)
            .and_then(|object| object.get("env"));
        let binding = if let Some(env) = env {
            let environment = env
                .as_str()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| invalid_error("env binding requires a variable name"))?;
            InputBinding {
                value: None,
                sources: Vec::new(),
                optional: false,
                status: if available_environment.contains(environment) {
                    RequirementStatus::Supplied
                } else {
                    RequirementStatus::Missing
                },
                environment: Some(environment.into()),
                unresolved: None,
            }
        } else if wrapped {
            InputBinding::concrete(
                value
                    .get("value")
                    .cloned()
                    .ok_or_else(|| invalid_error("binding requires value or env"))?,
            )
        } else {
            InputBinding::concrete(value.clone())
        };
        result.insert(name.clone(), binding);
    }
    Ok(result)
}

impl LocalConfiguredRuntime {
    fn selected_inputs(&self, available: &BTreeSet<String>) -> CwrResult<InputBindings> {
        self.select_inputs(available, None)
    }

    fn select_inputs(&self, available: &BTreeSet<String>, inspection: Option<&mut InputInspection>) -> CwrResult<InputBindings> {
        if let Some(mappings) = self.inputs.as_array() {
            let (mut parent, report) = requirements::workspace_input_contract(
                &self.workspace_path,
                &self.workspace_inputs,
                host_bindings(&self.workspace_bindings, available, true)?,
            )?;
            if report
                .issues
                .iter()
                .any(|issue| issue.message.starts_with("duplicate input"))
            {
                return invalid("duplicate workspace input declaration");
            }
            parent.retain(|name, _| self.workspace_inputs.iter().any(|input| &input.alias == name));
            if let Some(report) = inspection {
                return Ok(inspect_input_selections(
                    mappings.iter().map(|mapping| requirements::forward_bindings(&json!([mapping]), &parent)),
                    &self.cwr_toml, report,
                ));
            }
            requirements::forward_bindings(&self.inputs, &parent)
        } else {
            if let (Some(report), Some(inputs)) = (inspection, self.inputs.as_object()) {
                return Ok(inspect_input_selections(
                    inputs.iter().map(|(name, value)| host_bindings(&json!({name: value}), available, false)),
                    &self.cwr_toml, report,
                ));
            }
            host_bindings(&self.inputs, available, false)
        }
    }

    fn volume_requirements(&self) -> CwrResult<(Vec<Requirement>, Vec<RequirementIssue>)> {
        let declarations = requirements::inspect_volume_declarations(&self.cwr_toml)?;
        let scope = self.cwr_toml.to_string_lossy().into_owned();
        let mut rows = Vec::new();
        let mut issues = Vec::new();
        let mut problem = |status, message: String| {
            issues.push(RequirementIssue {
                scope: scope.clone(),
                status,
                message,
            })
        };
        let mut aliases = BTreeSet::new();
        for mapping in &self.volume_mappings {
            if !aliases.insert(&mapping.alias) {
                problem(
                    RequirementStatus::Invalid,
                    format!("duplicate volume mapping {}", mapping.alias),
                );
            }
            if !declarations
                .iter()
                .any(|declared| declared.alias == mapping.alias)
            {
                problem(
                    RequirementStatus::Invalid,
                    format!("undeclared volume destination {}", mapping.alias),
                );
            }
        }
        let mut declared_names = BTreeSet::new();
        for declaration in declarations {
            if !declared_names.insert(declaration.alias.clone()) {
                problem(
                    RequirementStatus::Invalid,
                    format!("duplicate volume declaration {}", declaration.alias),
                );
            }
            let mapping = self
                .volume_mappings
                .iter()
                .find(|mapping| mapping.alias == declaration.alias);
            let volume = mapping
                .and_then(|mapping| mapping.reference.strip_prefix("~@/"))
                .and_then(|name| {
                    self.workspace_volumes
                        .iter()
                        .find(|volume| volume.name == name)
                });
            let mut status = match (mapping, volume) {
                (Some(_), None) => RequirementStatus::Invalid,
                (None, _) if declaration.required => RequirementStatus::Invalid,
                (_, Some(volume)) if volume.path.is_dir() => RequirementStatus::Supplied,
                _ => RequirementStatus::Missing,
            };
            if let (Some(mapping), Some(volume)) = (mapping, volume) {
                if !access_allows(volume.access, mapping.access)
                    || (declaration.access == "read"
                        && mapping.access != LocalVolumeAccessMode::Read)
                    || (declaration.access == "read_write"
                        && mapping.access != LocalVolumeAccessMode::ReadWrite)
                {
                    status = RequirementStatus::Invalid;
                }
            }
            if status == RequirementStatus::Invalid {
                problem(
                    status,
                    format!(
                        "volume {} requires a valid explicit mapping and sufficient access",
                        declaration.alias
                    ),
                );
            }
            if status == RequirementStatus::Missing && declaration.required {
                problem(
                    status,
                    format!("volume {} needs a folder binding", declaration.alias),
                );
            }
            rows.push(Requirement {
                scope: scope.clone(),
                alias: declaration.alias,
                type_ref: "volume".into(),
                purpose: declaration.purpose,
                required: declaration.required,
                status,
                runtime_name: Some(self.name.clone()),
                sources: volume
                    .map(|volume| {
                        vec![requirements::InputSource {
                            scope: self.workspace_path.to_string_lossy().into_owned(),
                            alias: volume.name.clone(),
                            type_ref: "volume".into(),
                            storage: None,
                        }]
                    })
                    .unwrap_or_default(),
                requested_access: Some(declaration.access),
                available_access: volume.map(|volume| access_name(volume.access).into()),
            });
        }
        // Existing primary/additional attachments are explicit workspace consumers too.
        for attachment in &self.volume_access {
            let volume = self
                .workspace_volumes
                .iter()
                .find(|volume| volume.name == attachment.volume);
            let status = match volume {
                Some(volume) if !access_allows(volume.access, attachment.access) => {
                    RequirementStatus::Invalid
                }
                Some(volume) if volume.path.is_dir() => RequirementStatus::Supplied,
                _ => RequirementStatus::Missing,
            };
            if status != RequirementStatus::Supplied {
                problem(
                    status,
                    format!(
                        "volume {} needs an available folder and sufficient access",
                        attachment.volume
                    ),
                );
            }
            rows.push(Requirement {
                scope: scope.clone(),
                alias: attachment.volume.clone(),
                type_ref: "volume".into(),
                purpose: "Configured runtime attachment".into(),
                required: true,
                status,
                sources: vec![requirements::InputSource {
                    scope: self.workspace_path.to_string_lossy().into_owned(),
                    alias: attachment.volume.clone(),
                    type_ref: "volume".into(),
                    storage: None,
                }],
                runtime_name: Some(self.name.clone()),
                requested_access: Some(access_name(attachment.access).into()),
                available_access: volume.map(|volume| access_name(volume.access).into()),
            });
        }
        Ok((rows, issues))
    }
}

// An invalid mapping must not hide the other supplied inputs or declarations.
// Execution continues to use the strict selection path above.
fn inspect_input_selections(
    selections: impl Iterator<Item = CwrResult<InputBindings>>,
    path: &Path,
    report: &mut InputInspection,
) -> InputBindings {
    let mut inputs = InputBindings::new();
    let mut aliases = BTreeSet::new();
    for selection in selections {
        let result = selection.and_then(|bindings| {
            for (name, binding) in bindings {
                if !aliases.insert(name.clone()) {
                    inputs.remove(&name);
                    return invalid(format!("duplicate input mapping {name}"));
                }
                inputs.insert(name, binding);
            }
            Ok(())
        });
        if let Err(error) = result {
            report.issues.push(RequirementIssue {
                scope: path.to_string_lossy().into_owned(),
                status: RequirementStatus::Invalid,
                message: error.to_string(),
            });
        }
    }
    inputs
}

fn access_allows(grant: LocalVolumeAccessMode, requested: LocalVolumeAccessMode) -> bool {
    grant == LocalVolumeAccessMode::ReadWrite || requested == LocalVolumeAccessMode::Read
}
fn access_name(access: LocalVolumeAccessMode) -> &'static str {
    match access {
        LocalVolumeAccessMode::Read => "read",
        LocalVolumeAccessMode::ReadWrite => "read_write",
    }
}
