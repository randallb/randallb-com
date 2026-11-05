use super::*;

pub(in crate::v2026_09::graph) fn check_inputs(
    source: &Path,
    base: &ManifestFile,
    mut inputs: InputBindings,
    resolver: &dyn CheckResourceResolver,
    inspection: Option<&mut InputInspection>,
    root: bool,
) -> CwrResult<InputBindings> {
    let inspecting = inspection.is_some();
    let mut report = InputInspection::default();
    let scope = source.to_string_lossy().into_owned();
    let mut problem = |status, message: String| {
        report.issues.push(RequirementIssue {
            scope: scope.clone(),
            status,
            message,
        });
    };
    if base.input_schema.is_some() && !base.inputs.is_empty() {
        problem(
            RequirementStatus::Invalid,
            "use typed inputs or input_schema, not both".into(),
        );
    }
    let mut names = BTreeSet::new();
    for declaration in &base.inputs {
        let name = &declaration.alias;
        if !names.insert(name.clone()) {
            problem(
                RequirementStatus::Invalid,
                format!("duplicate input declaration {name}"),
            );
            continue;
        }
        let mapped = inputs.contains_key(name);
        let input = inputs
            .entry(name.clone())
            .or_insert_with(InputBinding::missing);
        input.optional = !declaration.required;
        if !matches!(declaration.type_ref.as_str(), PLAINTEXT_TYPE | SECRET_TYPE) {
            input.status = RequirementStatus::Unsupported;
            problem(
                RequirementStatus::Unsupported,
                format!(
                    "unsupported input type for {name}: {}",
                    declaration.type_ref
                ),
            );
        } else if input
            .sources
            .iter()
            .any(|origin| origin.type_ref != declaration.type_ref)
        {
            input.status = RequirementStatus::Invalid;
            problem(
                RequirementStatus::Invalid,
                format!("input {name} forwarding must preserve its declared type"),
            );
        } else if let Some(value) = &input.value {
            let schema: Value = serde_json::from_str(if declaration.type_ref == SECRET_TYPE {
                include_str!(
                    "../../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/secret/schema.json"
                )
            } else {
                include_str!(
                    "../../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/plaintext/schema.json"
                )
            })?;
            if !json_schema_validator(&schema)
                .map_err(|_| invalid_error("invalid input type schema"))?
                .is_valid(value)
            {
                input.status = RequirementStatus::Invalid;
                problem(
                    RequirementStatus::Invalid,
                    format!("input {name} does not satisfy its declared type"),
                );
            }
        }
        if !mapped && declaration.required && !root {
            input.status = RequirementStatus::Invalid;
            problem(
                RequirementStatus::Invalid,
                format!("required input {name} has no mapping"),
            );
        }
        if !inspecting && input.value.is_none() && declaration.required {
            problem(
                RequirementStatus::Missing,
                format!("required input {name} is unavailable"),
            );
        }
        let origins = input.sources.clone();
        if input.sources.is_empty() {
            input.sources.push(InputSource {
                scope: scope.clone(),
                alias: name.clone(),
                type_ref: declaration.type_ref.clone(),
                storage: declaration.storage,
            });
            // This declaration introduced the source, including literal bindings.
            input.unresolved = None;
        }
        report.requirements.push(Requirement {
            scope: scope.clone(),
            alias: name.clone(),
            type_ref: declaration.type_ref.clone(),
            purpose: declaration.purpose.clone(),
            required: declaration.required,
            status: input.status,
            sources: origins,
            runtime_name: None,
            requested_access: None,
            available_access: None,
        });
    }
    if let Some(locator) = &base.input_schema {
        let has_secret = inputs.values().any(|input| {
            !input.omitted()
                && input.sources.iter().any(|origin| {
                    origin.type_ref == SECRET_TYPE
                        || origin.storage == Some(PlaintextStorage::WorkspaceVariable)
                })
        });
        if has_secret {
            problem(RequirementStatus::Invalid, "secret input cannot cross an opaque input_schema boundary (workspace variables also require typed forwarding); declare typed inputs on the receiver".into());
        }
        let schema = load_schema_locator(
            source,
            "configuration input schema",
            locator,
            None,
            Some(&resolver::canonical_source_ref(source)),
            resolver,
        )?;
        if inspecting {
            problem(RequirementStatus::Unsupported, "opaque input_schema boundary: requirements cannot be inspected; concrete values are validated at execution".into());
        } else {
            let values = binding_values(&inputs);
            let validator = json_schema_validator(&schema)
                .map_err(|_| invalid_error("invalid configuration input schema"))?;
            if let Some(error) = validator.iter_errors(&values).next() {
                problem(
                    RequirementStatus::Invalid,
                    format!(
                        "configuration inputs do not satisfy schema at {}",
                        error.instance_path()
                    ),
                );
            };
        }
    } else {
        for name in inputs.keys() {
            if !names.contains(name) {
                problem(
                    RequirementStatus::Invalid,
                    format!("input {name} supplied without a declaration"),
                );
            }
        }
    }
    if let Some(inspection) = inspection {
        inspection.requirements.extend(report.requirements);
        inspection.issues.extend(report.issues);
    } else if let Some(issue) = report.issues.first() {
        return invalid(format!("{}: {}", source.display(), issue.message));
    }
    Ok(inputs)
}
