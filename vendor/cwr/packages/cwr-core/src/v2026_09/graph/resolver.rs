use super::requirements::{
    bind_template, check_inputs, concrete_bindings, equivalent_inputs, forward_bindings,
    InputBindings, InputEquivalence, InputInspection,
};
use super::*;
use sha2::{Digest, Sha256};

#[path = "dependencies.rs"]
mod dependencies;

/// File identity is independent of contents, import order, and aliases.
pub(super) fn canonical_source_ref(source: &Path) -> String {
    format!(
        "cwp://org.coworkerprotocol.local/resources/{}",
        hex::encode(Sha256::digest(source.as_os_str().as_encoded_bytes()))
    )
}

pub(super) fn resolve_manifest_graph(
    path: &Path,
    resolver: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
) -> CwrResult<ResolvedGraph> {
    load_manifest(
        path,
        resolver,
        options,
        concrete_bindings(&options.inputs)?,
        false,
        true,
    )
    .map(|loaded| loaded.graph)
}

pub(super) fn resolve_manifest_with_bindings(
    path: &Path,
    resolver: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
    inputs: InputBindings,
) -> CwrResult<ResolvedGraph> {
    load_manifest(path, resolver, options, inputs, false, true).map(|loaded| loaded.graph)
}

pub(super) fn inspect_manifest_inputs(
    path: &Path,
    resolver: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
    inputs: InputBindings,
    unbound_root: bool,
) -> CwrResult<InputInspection> {
    let mut options = options.clone();
    options.allow_partial = false;
    load_manifest(path, resolver, &options, inputs, true, unbound_root).map(|loaded| loaded.report)
}

pub(super) struct LoadedManifest {
    pub graph: ResolvedGraph,
    pub report: InputInspection,
    pub can_project: bool,
}

pub(super) fn load_manifest(
    path: &Path,
    resolver: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
    inputs: InputBindings,
    inspect: bool,
    unbound_root: bool,
) -> CwrResult<LoadedManifest> {
    let roots = source_roots(path, resolver, options)?;
    let mut imported_volumes = BTreeMap::new();
    for binding in &options.imported_volumes {
        let source = binding.source.canonicalize()?;
        let folder = binding.path.canonicalize()?;
        if !source.is_file() || !folder.is_dir() {
            return invalid("imported volume binding requires a source file and folder");
        }
        if !matches!(binding.access.as_str(), "read" | "read_write") {
            return invalid("imported volume binding access must be read or read_write");
        }
        let key = (source, binding.alias.clone());
        if imported_volumes
            .insert(key, (folder, binding.access.clone()))
            .is_some()
        {
            return invalid("duplicate imported volume binding");
        }
    }
    let working_path = options
        .working_path
        .clone()
        .unwrap_or(std::env::current_dir()?)
        .canonicalize()?;
    if !working_path.is_dir() {
        return invalid("working_path must select a directory");
    }
    let mut loader = Loader {
        host: resolver,
        allow_partial: options.allow_partial,
        disabled: BTreeSet::new(),
        receiver_failed_sources: BTreeSet::new(),
        warnings: Vec::new(),
        roots,
        working_path,
        resources: Vec::new(),
        scopes: Vec::new(),
        by_source: BTreeMap::new(),
        edges: Vec::new(),
        pending: Vec::new(),
        bindings: Vec::new(),
        inspection: inspect.then(InputInspection::default),
        unbound_root,
        incomplete_params: false,
        imported_volumes,
        used_imported_volumes: BTreeSet::new(),
        resolved_volumes: BTreeMap::new(),
    };
    let root_index = loader.load_file(path, inputs, Visibility::Export)?;
    if let Err(error) = loader.resolve_pending() {
        if let Some(report) = &mut loader.inspection {
            report.issues.push(requirements::RequirementIssue {
                scope: path.to_string_lossy().into_owned(),
                status: requirements::RequirementStatus::Invalid,
                message: error.to_string(),
            });
        } else {
            return Err(error);
        }
    }
    if loader.has_unmatched_volume_grant() {
        return invalid("imported volume binding does not match a loaded declaration");
    }
    let mut report = loader.inspection.take().unwrap_or_default();
    let mut seen_issues = BTreeSet::new();
    report
        .issues
        .retain(|issue| seen_issues.insert((issue.scope.clone(), issue.message.clone())));
    // Unread bindings and incomplete relationships prevent projection.
    let can_project = !loader.incomplete_params
        && (report.issues.is_empty()
            || (report
                .issues
                .iter()
                .all(|issue| issue.status == requirements::RequirementStatus::Unsupported)
                && loader
                    .bindings
                    .iter()
                    .flatten()
                    .all(|(_, input)| input.value.is_some() || input.omitted())));
    Ok(LoadedManifest {
        graph: loader.finish(root_index)?,
        report,
        can_project,
    })
}

struct Loader<'a> {
    host: &'a dyn CheckResourceResolver,
    allow_partial: bool,
    disabled: BTreeSet<PathBuf>,
    receiver_failed_sources: BTreeSet<PathBuf>,
    warnings: Vec<GraphLoadWarning>,
    roots: Vec<PathBuf>,
    working_path: PathBuf,
    resources: Vec<LoadedResource>,
    scopes: Vec<ResolvedScope>,
    by_source: BTreeMap<PathBuf, usize>,
    edges: Vec<(usize, usize, Option<String>, Visibility)>,
    pending: Vec<(usize, ResourceObject, Visibility)>,
    bindings: Vec<InputBindings>,
    inspection: Option<InputInspection>,
    unbound_root: bool,
    incomplete_params: bool,
    imported_volumes: BTreeMap<(PathBuf, String), (PathBuf, String)>,
    used_imported_volumes: BTreeSet<(PathBuf, String)>,
    resolved_volumes: BTreeMap<PathBuf, BTreeMap<String, (PathBuf, String)>>,
}

impl Loader<'_> {
    fn load_file(
        &mut self,
        path: &Path,
        inputs: InputBindings,
        visibility: Visibility,
    ) -> CwrResult<usize> {
        let source = source_file(path, &self.roots)?;
        if self.disabled.contains(&source) {
            return invalid("dependency is disabled by a configuration error");
        }
        if let Some(index) = self.by_source.get(&source).copied() {
            self.check_reused_inputs(index, &inputs)?;
            return Ok(index);
        }
        let document = read_toml_json(&source).map_err(|error| {
            if self.inspection.is_some() {
                invalid_error(format!(
                    "{}: could not read a valid TOML declaration",
                    source.display()
                ))
            } else {
                error
            }
        })?;
        let type_ref = document_type(&source, &document)?.to_owned();
        let shared_state = matches!(
            type_ref.as_str(),
            CONVERSATION_TYPE | ACTION_INVOCATION_TYPE
        );
        self.load_declared_file(source, document, type_ref, inputs, visibility)
            .map_err(|error| {
                if shared_state {
                    Box::new(SharedStateLoadError(error.to_string()))
                        as Box<dyn std::error::Error + Send + Sync>
                } else {
                    error
                }
            })
    }

    fn load_declared_file(
        &mut self,
        source: PathBuf,
        document: Value,
        type_ref: String,
        inputs: InputBindings,
        visibility: Visibility,
    ) -> CwrResult<usize> {
        validate_core_document(&source, &type_ref, &document)?;
        if matches!(type_ref.as_str(), ACTOR_TYPE | ACTOR_INTEGRATION_TYPE) {
            return invalid(format!("{}: standalone actor declarations have been replaced by COWORKER.toml and named integrations", source.display()));
        }
        let base: ManifestFile = decode(&source, document.clone())?;
        let restricted = SourceResolver {
            host: self.host,
            roots: &self.roots,
        };
        let inputs = check_inputs(
            &source,
            &base,
            inputs,
            &restricted,
            self.inspection.as_mut(),
            self.unbound_root && self.resources.is_empty(),
        )
        .map_err(receiver_input_error)?;
        self.bind_imported_volumes(&source, &base.volumes)?;
        if self.inspection.is_none()
            && self.unbound_root
            && self.resources.is_empty()
            && base.volumes.iter().any(|volume| volume.required)
        {
            return invalid(
                "required volumes need an explicit workspace runtime selection and valid mappings",
            );
        }
        if let Some(report) = &mut self.inspection {
            if self.unbound_root || !self.resources.is_empty() {
                for volume in &base.volumes {
                    let status = if self.resources.is_empty() {
                        requirements::RequirementStatus::Missing
                    } else if self
                        .used_imported_volumes
                        .contains(&(source.clone(), volume.alias.clone()))
                    {
                        requirements::RequirementStatus::Supplied
                    } else {
                        requirements::RequirementStatus::Missing
                    };
                    report.requirements.push(requirements::Requirement {
                        scope: source.to_string_lossy().into_owned(),
                        alias: volume.alias.clone(),
                        type_ref: "volume".into(),
                        purpose: volume.purpose.clone(),
                        required: volume.required,
                        status,
                        sources: Vec::new(),
                        runtime_name: None,
                        requested_access: Some(volume.access.clone()),
                        available_access: None,
                    });
                    if status == requirements::RequirementStatus::Missing && volume.required {
                        report.issues.push(requirements::RequirementIssue {
                            scope: source.to_string_lossy().into_owned(),
                            status,
                            message: format!(
                                "volume {} needs an explicit folder binding",
                                volume.alias
                            ),
                        });
                    }
                }
            }
        }
        let index = self.resources.len();
        let reference = declared_resource_ref(&source, &type_ref, &document);
        if self
            .resources
            .iter()
            .any(|resource| resource.effective_ref == reference)
        {
            return Err(Box::new(NamespaceConflict(format!(
                "{}: duplicate canonical ref {reference}",
                source.display()
            ))));
        }
        let mut resource = load_resource(
            source.clone(),
            &type_ref,
            document,
            &reference,
            index,
            visibility,
        )?;
        self.bind_resource_params(&mut resource, &inputs, base.input_schema.is_some())?;
        self.by_source.insert(source.clone(), index);
        self.scopes.push(ResolvedScope {
            source: source.clone(),
            root_ref: reference.clone(),
            lookup: BTreeMap::from([(reference.clone(), ResolvedTarget::Resource(index))]),
        });
        self.resources.push(resource);
        self.bindings.push(inputs);
        let mut dependencies = base
            .imports
            .into_iter()
            .map(|locator| (locator, Visibility::Import))
            .chain(
                base.exports
                    .into_iter()
                    .map(|locator| (locator, Visibility::Export)),
            )
            .collect::<Vec<_>>();
        let relationships = match &self.resources[index].kind {
            ResourceKind::Coworker(coworker) => coworker
                .roles
                .iter()
                .cloned()
                .chain(
                    coworker
                        .integrations
                        .iter()
                        .map(|binding| binding.locator.clone()),
                )
                .collect(),
            ResourceKind::Role(role) => role
                .adapters
                .iter()
                .map(|binding| binding.locator.clone())
                .collect(),
            ResourceKind::Integration(integration) => vec![integration.adapter.clone()],
            ResourceKind::Gateway(gateway) => {
                vec![gateway.role.clone(), gateway.integration.clone()]
            }
            _ => Vec::new(),
        };
        dependencies.extend(
            relationships
                .into_iter()
                .map(|locator| (locator, Visibility::Import)),
        );
        // Index the entire path closure before any ref-only edge needs its target.
        for (locator, visibility) in dependencies {
            if locator.path.is_some() {
                if let Err(error) = self.load_locator(index, &locator, visibility) {
                    if is_fatal_load_error(error.as_ref()) {
                        return Err(error);
                    }
                    if self.allow_partial && index != 0 {
                        self.warn(source.clone(), "A configured dependency is unavailable; other bindings can still be used.");
                        continue;
                    }
                    if let Some(report) = &mut self.inspection {
                        report.issues.push(requirements::RequirementIssue {
                            scope: source.to_string_lossy().into_owned(),
                            status: requirements::RequirementStatus::Invalid,
                            message: error.to_string(),
                        });
                    } else {
                        return Err(error);
                    }
                }
            } else {
                self.pending.push((index, locator, visibility));
            }
        }
        Ok(index)
    }

    fn bind_imported_volumes(
        &mut self,
        source: &Path,
        volumes: &[requirements::VolumeDeclaration],
    ) -> CwrResult<()> {
        if self.resources.is_empty() {
            return Ok(());
        }
        for volume in volumes {
            let key = (source.to_path_buf(), volume.alias.clone());
            match self.imported_volumes.get(&key) {
                Some((folder, access)) if access == &volume.access => {
                    self.used_imported_volumes.insert(key);
                    self.resolved_volumes
                        .entry(source.to_path_buf())
                        .or_default()
                        .insert(volume.alias.clone(), (folder.clone(), access.clone()));
                }
                Some(_) => {
                    return invalid(format!(
                        "{}: volume {} binding has mismatched access",
                        source.display(),
                        volume.alias
                    ))
                }
                None if volume.required && self.inspection.is_none() => {
                    return invalid(format!(
                        "{}: volume {} needs an explicit folder binding",
                        source.display(),
                        volume.alias
                    ))
                }
                None => {}
            }
        }
        Ok(())
    }

    fn check_reused_inputs(&mut self, index: usize, inputs: &InputBindings) -> CwrResult<()> {
        let source = self.resources[index].source.clone();
        let base: ManifestFile = decode(&source, self.resources[index].document.clone())?;
        let restricted = SourceResolver {
            host: self.host,
            roots: &self.roots,
        };
        let mut repeated = InputInspection::default();
        let checked = check_inputs(
            &source,
            &base,
            inputs.clone(),
            &restricted,
            self.inspection.as_ref().map(|_| &mut repeated),
            false,
        )
        .map_err(receiver_input_error)?;
        if let Some(inspection) = &mut self.inspection {
            inspection.issues.extend(repeated.issues);
        }
        match equivalent_inputs(&self.bindings[index], &checked) {
            InputEquivalence::Equal => {}
            InputEquivalence::Conflicting => {
                return Err(Box::new(NamespaceConflict(format!(
                    "{}: conflicting effective inputs for the same resource",
                    source.display()
                ))))
            }
            InputEquivalence::Unresolved => {
                let message = "shared resource input equivalence requires concrete values";
                if let Some(inspection) = &mut self.inspection {
                    inspection.issues.push(requirements::RequirementIssue {
                        scope: source.to_string_lossy().into_owned(),
                        status: requirements::RequirementStatus::Unsupported,
                        message: message.into(),
                    });
                } else {
                    return invalid(message);
                }
            }
        }
        Ok(())
    }

    fn bind_resource_params(
        &mut self,
        resource: &mut LoadedResource,
        inputs: &InputBindings,
        opaque: bool,
    ) -> CwrResult<()> {
        let mut bind = |params: &mut Parameters, pointer: &str| -> CwrResult<()> {
            self.bind_params(params, inputs, opaque)?;
            if let Some(document) = resource.document.pointer_mut(pointer) {
                *document = params.value.clone();
            }
            Ok(())
        };
        match &mut resource.kind {
            ResourceKind::Integration(integration) => bind(&mut integration.params, "/params")?,
            ResourceKind::Coworker(coworker) => {
                for (index, binding) in coworker.integrations.iter_mut().enumerate() {
                    bind(
                        &mut binding.params,
                        &format!("/integrations/{index}/params"),
                    )?;
                }
            }
            ResourceKind::Role(role) => {
                for (index, binding) in role.adapters.iter_mut().enumerate() {
                    bind(&mut binding.params, &format!("/adapters/{index}/params"))?;
                }
            }
            ResourceKind::Gateway(gateway) => bind(&mut gateway.params, "/params")?,
            _ => {}
        }
        Ok(())
    }

    fn bind_params(
        &mut self,
        params: &mut Parameters,
        inputs: &InputBindings,
        opaque: bool,
    ) -> CwrResult<()> {
        match bind_template(&params.value, inputs) {
            Ok(binding) if binding.omitted() => *params = Parameters::default(),
            Ok(binding) => {
                if binding.value.is_none() && self.inspection.is_none() {
                    return invalid("required parameter input is unavailable");
                }
                *params = binding.inspection_value();
            }
            Err(error) if self.inspection.is_some() => {
                *params = Parameters::default();
                self.incomplete_params = true;
                if !opaque {
                    self.inspection
                        .as_mut()
                        .unwrap()
                        .issues
                        .push(requirements::RequirementIssue {
                            scope: "parameters".into(),
                            status: requirements::RequirementStatus::Invalid,
                            message: error.to_string(),
                        });
                }
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }
}

pub(super) fn source_roots(
    path: &Path,
    resolver: &dyn CheckResourceResolver,
    options: &GraphLoadOptions,
) -> CwrResult<Vec<PathBuf>> {
    let root = path
        .parent()
        .ok_or_else(|| invalid_error("declaration has no parent"))?;
    let roots = if options.source_roots.is_empty() {
        resolver
            .source_roots()
            .map(<[PathBuf]>::to_vec)
            .unwrap_or_else(|| vec![root.to_owned()])
    } else {
        options.source_roots.clone()
    };
    roots
        .into_iter()
        .map(|root| root.canonicalize().map_err(Into::into))
        .collect()
}

pub(super) fn source_file(path: &Path, roots: &[PathBuf]) -> CwrResult<PathBuf> {
    let source = resolved_regular_file("source", path)?;
    if !roots.iter().any(|root| source.starts_with(root)) {
        return invalid(format!(
            "{}: source is outside the workspace source roots",
            path.display()
        ));
    }
    Ok(source)
}

pub(super) struct SourceResolver<'a> {
    pub host: &'a dyn CheckResourceResolver,
    pub roots: &'a [PathBuf],
}
impl CheckResourceResolver for SourceResolver<'_> {
    fn source_roots(&self) -> Option<&[PathBuf]> {
        Some(self.roots)
    }
    fn resolve(&self, reference: &str) -> CwrResult<Option<PathBuf>> {
        self.host
            .resolve(reference)?
            .map(|path| source_file(&path, self.roots))
            .transpose()
    }
}

include!("load_errors.rs");
