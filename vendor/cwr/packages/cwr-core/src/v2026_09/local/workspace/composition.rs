use crate::v2026_09::graph::requirements::{self, InputDeclaration, InputBindings, InputBinding, InputInspection, Requirement, RequirementIssue, RequirementStatus};

/// A workspace composes independently configured runtimes over explicit persistent volumes.
#[derive(Debug, Clone)]
pub struct LocalWorkspaceComposition {
    pub path: PathBuf,
    pub volumes: Vec<LocalWorkspaceVolume>,
    pub runtimes: Vec<LocalConfiguredRuntime>,
    pub inputs: Vec<InputDeclaration>,
    bindings: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalWorkspaceVolume {
    pub name: String,
    #[serde(default)]
    pub path: PathBuf,
    #[serde(default)]
    pub access: LocalVolumeAccessMode,
    #[serde(default = "requirements::required_by_default")]
    pub required: bool,
    #[serde(default)]
    pub purpose: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalVolumeAccess {
    pub volume: String,
    pub access: LocalVolumeAccessMode,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalVolumeAccessMode {
    Read,
    #[default]
    ReadWrite,
}

#[derive(Debug, Clone)]
pub struct LocalConfiguredRuntime {
    pub name: String,
    pub cwr_toml: PathBuf,
    pub source_roots: Vec<PathBuf>,
    pub working_directory: PathBuf,
    pub inputs: Value,
    pub volume: String,
    pub volume_access: Vec<LocalVolumeAccess>,
    pub state_directory: PathBuf,
    pub runtime_ref: String,
    pub acting_coworker: Option<String>,
    pub allow_reconfiguration: bool,
    workspace_path: PathBuf,
    workspace_inputs: Vec<InputDeclaration>,
    workspace_bindings: Value,
    workspace_volumes: Vec<LocalWorkspaceVolume>,
    volume_mappings: Vec<VolumeMapping>,
    identity_runtime_id: String,
    identity_origin: ConfiguredRuntimeIdentityOrigin,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceCompositionFile {
    #[serde(rename = "type")]
    _type_ref: String,
    volumes: Vec<LocalWorkspaceVolume>,
    #[serde(alias = "loops")]
    runtimes: Vec<ConfiguredRuntimeFile>,
    #[serde(default)]
    inputs: Vec<InputDeclaration>,
    #[serde(default = "composition_empty_inputs")]
    bindings: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfiguredRuntimeFile {
    name: String,
    path: PathBuf,
    source_roots: Vec<PathBuf>,
    working_directory: PathBuf,
    volume: String,
    #[serde(default)]
    volume_access: Vec<LocalVolumeAccess>,
    #[serde(default = "composition_empty_inputs")]
    inputs: Value,
    acting_coworker: Option<String>,
    #[serde(default)]
    allow_reconfiguration: bool,
    #[serde(default)]
    volumes: Vec<VolumeMapping>,
}

fn composition_empty_inputs() -> Value {
    json!({})
}

struct CompositionResourceResolver;

impl crate::v2026_09::graph::CheckResourceResolver for CompositionResourceResolver {
    fn resolve(&self, _reference: &str) -> CwrResult<Option<PathBuf>> {
        Ok(None)
    }
}

impl LocalWorkspaceComposition {
    /// Load and check workspace-owned paths without opening or modifying runtime state.
    pub fn load(path: impl AsRef<Path>) -> CwrResult<Self> {
        let path = canonical_workspace_path(path.as_ref())?;
        let document: Value = toml::from_str(&fs::read_to_string(&path)?)
            .map_err(|_| invalid_error("invalid WORKSPACE.toml composition fields"))?;
        if document.get("version").is_some() {
            return invalid("WORKSPACE.toml version is retired; replace version = 1 with type = \"cwp://org.coworkerprotocol/2026-09/schemas/workspace\" and select a reusable runtime definition");
        }
        if document.get("type").and_then(Value::as_str) != Some(crate::v2026_09::graph::WORKSPACE_TYPE) {
            return invalid("missing or unsupported WORKSPACE.toml type; expected cwp://org.coworkerprotocol/2026-09/schemas/workspace");
        }
        crate::v2026_09::graph::validate_workspace_document(&path, &document)?;
        let file: WorkspaceCompositionFile = serde_json::from_value(document)
            .map_err(|_| invalid_error("invalid WORKSPACE.toml composition fields"))?;
        let base = path
            .parent()
            .ok_or_else(|| invalid_error("workspace has no directory"))?;
        let mut volumes = file.volumes;
        let mut volume_names = BTreeSet::new();
        for volume in &mut volumes {
            validate_composition_name(&volume.name)?;
            if !volume_names.insert(volume.name.clone()) {
                return invalid(format!("duplicate workspace volume {}", volume.name));
            }
            // An absent or unavailable folder is a setup requirement, not a load failure.
            if !volume.path.as_os_str().is_empty() {
                volume.path = base.join(&volume.path);
            }
        }
        let mut runtime_names = BTreeSet::new();
        let mut runtimes = Vec::new();
        for declared in file.runtimes {
            validate_composition_name(&declared.name)?;
            if !runtime_names.insert(declared.name.clone()) {
                return invalid(format!("duplicate configured runtime {}", declared.name));
            }
            let volume = volumes
                .iter()
                .find(|volume| volume.name == declared.volume)
                .ok_or_else(|| {
                    invalid_error(format!(
                        "runtime {} selects unknown volume {}",
                        declared.name, declared.volume
                    ))
                })?;
            let mut access = declared.volume_access;
            let mut seen_access = BTreeSet::new();
            for attachment in &access {
                if !volume_names.contains(&attachment.volume)
                    || !seen_access.insert(attachment.volume.clone())
                {
                    return invalid(format!(
                        "runtime {} has unknown or repeated volume attachment {}",
                        declared.name, attachment.volume
                    ));
                }
            }
            if !access.iter().any(|entry| entry.volume == volume.name) {
                access.push(LocalVolumeAccess {
                    volume: volume.name.clone(),
                    access: LocalVolumeAccessMode::ReadWrite,
                });
            }
            let cwr_toml = fs::canonicalize(base.join(&declared.path)).map_err(|error| crate::error::configuration_file_error(&base.join(&declared.path), error))?;
            if !fs::metadata(&cwr_toml)?.is_file() {
                return invalid(format!(
                    "runtime {} declaration must be a regular file",
                    declared.name
                ));
            }
            let source_roots = declared
                .source_roots
                .iter()
                .map(|source| composition_directory(base, source, "source root"))
                .collect::<CwrResult<Vec<_>>>()?;
            if source_roots.is_empty()
                || !source_roots.iter().any(|root| cwr_toml.starts_with(root))
            {
                return invalid(format!(
                    "runtime {} declaration is outside its source roots",
                    declared.name
                ));
            }
            let working_directory =
                composition_directory(base, &declared.working_directory, "working directory")?;
            crate::v2026_09::graph::validate_runtime_definition(&cwr_toml)?;
            let inputs = declared.inputs;
            // Context identity depends on workspace + selected runtime, never the imported source.
            // Storage retains its historical digest path even though fresh identities no longer
            // use the legacy authority or /loops terminology.
            let digest = configured_runtime_digest(&path, &declared.name);
            let state_directory = platform_state_home()?
                .join(application_directory_name())
                .join("loops")
                .join(&digest);
            let identity = select_configured_runtime_identity(&state_directory, &digest)?;
            runtimes.push(LocalConfiguredRuntime {
                name: declared.name,
                cwr_toml,
                source_roots,
                working_directory,
                inputs,
                volume: volume.name.clone(),
                volume_access: access,
                state_directory,
                runtime_ref: identity.runtime_root_ref,
                acting_coworker: declared.acting_coworker,
                allow_reconfiguration: declared.allow_reconfiguration,
                workspace_path: path.clone(),
                workspace_inputs: file.inputs.clone(),
                workspace_bindings: file.bindings.clone(),
                workspace_volumes: volumes.clone(),
                volume_mappings: declared.volumes,
                identity_runtime_id: digest,
                identity_origin: identity.origin,
            });
        }
        if runtimes.is_empty() {
            return invalid("workspace composition requires at least one configured runtime");
        }
        Ok(Self {
            path,
            volumes,
            runtimes,
            inputs: file.inputs,
            bindings: file.bindings,
        })
    }

    pub fn configured_runtime(&self, name: &str) -> CwrResult<&LocalConfiguredRuntime> {
        self.runtimes
            .iter()
            .find(|configured| configured.name == name)
            .ok_or_else(|| invalid_error(format!("unknown configured runtime {name}")))
    }

    pub fn check(&self) -> CwrResult<()> {
        for configured in &self.runtimes {
            configured.catalog()?;
        }
        Ok(())
    }
}

impl LocalConfiguredRuntime {
    pub fn workspace_path(&self) -> &Path {
        &self.workspace_path
    }

    pub fn runtime_directory(&self) -> CwrResult<PathBuf> {
        let key = self.runtime_ref.rsplit('/').next().expect("runtime digest");
        configured_runtime_runtime_directory_for_digest(key)
    }

    pub fn graph_options(&self) -> crate::v2026_09::graph::GraphLoadOptions {
        crate::v2026_09::graph::GraphLoadOptions {
            allow_partial: true,
            source_roots: self.source_roots.clone(),
            inputs: self.inputs.clone(),
            working_path: Some(self.working_directory.clone()),
            runtime_ref: Some(self.runtime_ref.clone()),
            input_resolution: Default::default(),
            imported_volumes: Vec::new(),
        }
    }

    pub(super) fn graph_options_with_imported_volumes(&self) -> CwrResult<crate::v2026_09::graph::GraphLoadOptions> {
        let mut options = self.graph_options();
        // A configured workspace opens the same parent entry as an ordinary workspace.
        options.imported_volumes = crate::v2026_09::local::imported_volumes::load(&self.cwr_toml)?;
        Ok(options)
    }

    pub fn resolve_graph(&self) -> CwrResult<ResolvedCwrGraph> {
        self.prepare().map(|(graph, _)| graph)
    }

    fn resolved_host_inputs(&self) -> CwrResult<InputBindings> {
        let mut inputs = self.selected_inputs(&BTreeSet::new())?;
        for binding in inputs.values_mut() {
            if let Some(environment) = binding.environment.take() {
                binding.value = std::env::var(&environment).ok().map(Value::String);
                binding.unresolved = None;
                binding.status = if binding.value.is_some() { RequirementStatus::Supplied } else { RequirementStatus::Missing };
            }
        }
        Ok(inputs)
    }

    pub(super) fn connection_options(&self) -> CwrResult<(crate::v2026_09::graph::GraphLoadOptions, BTreeSet<String>)> {
        let inputs = self.resolved_host_inputs()?;
        let mut options = self.graph_options_with_imported_volumes()?;
        options.inputs = requirements::binding_values(&inputs);
        Ok((options, inputs.iter().filter(|(_, b)| b.value.is_some()).map(|(alias, _)| alias.clone()).collect()))
    }

    fn prepare(&self) -> CwrResult<(ResolvedCwrGraph, crate::v2026_09::graph::GraphLoadOptions)> {
        crate::v2026_09::graph::validate_runtime_definition(&self.cwr_toml)?;
        let (_, issues) = self.volume_requirements()?;
        if let Some(issue) = issues.first() { return invalid(&issue.message); }
        let target = connections::configured_target(self)?;
        if !connections::uses_managed_setup(&target)? {
            let inputs = self.resolved_host_inputs()?;
            let options = target.options;
            let graph = crate::v2026_09::graph::resolve_bound_graph(&self.cwr_toml, &options, inputs)?;
            return Ok((graph, options));
        }
        let options = connections::options_for_target(&target)?;
        // Preserve forwarding provenance for supplied host inputs. Managed
        // assignments and local JSON originate at the runtime declaration.
        let mut inputs = self.resolved_host_inputs()?;
        let managed: BTreeSet<_> = requirements::inspect_input_declarations(&self.cwr_toml)?.into_iter()
            .filter(|d| d.type_ref == requirements::SECRET_TYPE || d.storage == Some(requirements::PlaintextStorage::WorkspaceVariable)).map(|d| d.alias).collect();
        for (alias, value) in options.inputs.as_object().ok_or("Expected configured inputs.")? {
            if managed.contains(alias) || options.input_resolution.selections.contains_key(alias) || inputs.get(alias).and_then(|b| b.value.as_ref()) != Some(value) {
                inputs.insert(alias.clone(), InputBinding::concrete(value.clone()));
            }
        }
        let graph = crate::v2026_09::graph::resolve_bound_graph(&self.cwr_toml, &options, inputs)?;
        Ok((graph, options))
    }

    pub fn database_path(&self) -> PathBuf {
        self.state_directory.join("cwpdb.sqlite3")
    }

    pub(super) fn persist_identity_before_database(&self) -> CwrResult<()> {
        if self.identity_origin != ConfiguredRuntimeIdentityOrigin::Fresh {
            return Ok(());
        }
        if !configured_runtime_identity_path(&self.state_directory).try_exists()?
            && self.database_path().try_exists()?
        {
            return invalid(
                "configured runtime state appeared while opening; reload the workspace before retrying",
            );
        }
        persist_configured_runtime_identity(
            &self.state_directory,
            &self.identity_runtime_id,
            &self.runtime_ref,
        )
    }

    pub(super) fn persist_identity_after_database(&self) -> CwrResult<()> {
        if self.identity_origin != ConfiguredRuntimeIdentityOrigin::LegacyDatabase {
            return Ok(());
        }
        persist_configured_runtime_identity(
            &self.state_directory,
            &self.identity_runtime_id,
            &self.runtime_ref,
        )
    }

    pub fn host_identity(&self, ingress_ref: &str) -> CwrResult<LocalHostIdentity> {
        let graph = self.resolve_graph()?;
        Ok(LocalHostIdentity::new(
            self.acting_actor_ref(&graph)?,
            ingress_ref,
        ))
    }

    /// Validate activation and inspect available coworkers without opening state.
    pub fn catalog(&self) -> CwrResult<LocalCatalog> {
        let graph = self.resolve_graph()?;
        let acting_actor_ref = self.acting_actor_ref(&graph)?;
        let graph = runtime_graph(graph, &self.cwr_toml)?;
        local_catalog(&graph, &acting_actor_ref)
    }

    fn acting_actor_ref(&self, graph: &ResolvedCwrGraph) -> CwrResult<String> {
        match self.acting_coworker.as_deref() {
            Some(requested) => {
                let matches = graph
                    .actors
                    .iter()
                    .filter(|actor| {
                        actor.actor_ref == requested
                            || actor.coworker_slug == requested.trim_start_matches('@')
                    })
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    return invalid(format!(
                        "runtime {} acting_coworker must select one loaded coworker",
                        self.name
                    ));
                }
                Ok(matches[0].actor_ref.clone())
            }
            None => default_local_acting_actor_ref(graph),
        }
    }

    /// Opening a configured runtime activates its graph; it never adopts another runtime's state.
    pub fn open(&self, ingress_ref: &str) -> CwrResult<LocalRuntime> {
        let (graph, options) = self.prepare()?;
        let identity = LocalHostIdentity::new(self.acting_actor_ref(&graph)?, ingress_ref);
        LocalRuntime::open_prepared_runtime(
            self,
            identity,
            LocalRuntimeOptions::configured()?,
            graph,
            options,
        )
    }
}

/// Locate one configured runtime's resident without validating its edited composition.
pub fn configured_runtime_runtime_directory(
    workspace: impl AsRef<Path>,
    name: &str,
) -> CwrResult<PathBuf> {
    validate_composition_name(name)?;
    let workspace = canonical_workspace_path(workspace.as_ref())?;
    configured_runtime_runtime_directory_for_digest(&configured_runtime_digest(&workspace, name))
}

fn canonical_workspace_path(requested: &Path) -> CwrResult<PathBuf> {
    let requested = if requested.is_dir() {
        requested.join("WORKSPACE.toml")
    } else {
        requested.to_owned()
    };
    let path = fs::canonicalize(requested)?;
    if !fs::metadata(&path)?.is_file() {
        return invalid("workspace composition must be a regular file");
    }
    Ok(path)
}

fn configured_runtime_digest(workspace: &Path, name: &str) -> String {
    let key = format!("{}\n{name}", workspace.display());
    hex::encode(Sha256::digest(key.as_bytes()))
}

fn configured_runtime_runtime_directory_for_digest(digest: &str) -> CwrResult<PathBuf> {
    Ok(platform_runtime_home()?
        .join("cwr/loops")
        .join(&digest[..32]))
}

fn validate_composition_name(name: &str) -> CwrResult<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return invalid(
            "workspace runtime and volume names require letters, digits, underscores, or hyphens",
        );
    }
    Ok(())
}

fn composition_directory(base: &Path, requested: &Path, label: &str) -> CwrResult<PathBuf> {
    let path = fs::canonicalize(base.join(requested)).map_err(|error| crate::error::configuration_file_error(&base.join(requested), error))?;
    if !fs::metadata(&path)?.is_dir() {
        return invalid(format!("workspace {label} must be a directory"));
    }
    Ok(path)
}

include!("prepared.rs");
include!("requirements.rs");
include!("bindings.rs");

#[cfg(test)]
#[path = "composition_tests.rs"]
mod composition_tests;

#[cfg(test)]
#[path = "requirements_tests.rs"]
mod workspace_requirements_tests;
